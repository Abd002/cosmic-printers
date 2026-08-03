use crate::backend::Backend;
use cosmic::app::Task;
use cosmic::iced::core::text::{Ellipsize, EllipsizeHeightLimit, Wrapping};
use cosmic::iced::{
    Alignment, Color, Length, Point,
    futures::{SinkExt, StreamExt, channel::mpsc::Sender, future},
    stream,
};
use cosmic::surface;
use cosmic::widget::{self, column, container, menu, row, settings, text};
use cosmic::{Apply, Element};
use cosmic_settings_printers_core::{GroupedDestination, group_printers};
pub use cosmic_settings_printers_core::{
    JobFilter, PrinterApplication, PrinterEntry, PrinterStatus, PrintersEvent, PrintersEventKind,
    SupplyLevel,
};
use std::collections::HashMap;

const CONTEXT_MENU_WIDTH: f32 = 360.0;
const CONTEXT_MENU_ROW_HEIGHT: f32 = 40.0;

/// State for the printer list and Add Printer dialog.
pub struct State {
    backend: Backend,
    pub(crate) printers: Vec<PrinterEntry>,
    printer_applications: Vec<PrinterApplication>,
    pub(crate) default_printer_id: Option<String>,
    active_job_counts: HashMap<String, usize>,
    pub(crate) add_printer_dialog: Option<crate::add_printer::State>,
    default_printer_labels: Vec<String>,
    printer_context: Option<String>,
    menu_position: Point,
    cursor_position: Point,
}

impl State {
    /// Returns the active Add Printer dialog.
    #[must_use]
    pub fn add_printer_dialog(&self) -> Option<&crate::add_printer::State> {
        self.add_printer_dialog.as_ref()
    }

    /// Creates the initial printer-loading task.
    pub fn load_task<M>(&self) -> Task<M>
    where
        M: 'static + Send + From<Message>,
    {
        self.load_printers_task()
    }

    /// Sets the backend for the list and active dialog.
    pub fn set_backend(&mut self, backend: Backend) {
        self.backend = backend.clone();
        if let Some(dialog) = self.add_printer_dialog.as_mut() {
            dialog.set_backend(backend);
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            backend: Backend::default(),
            printers: Vec::new(),
            printer_applications: Vec::new(),
            default_printer_id: None,
            active_job_counts: HashMap::new(),
            add_printer_dialog: None,
            default_printer_labels: default_printer_labels(&[]),
            printer_context: None,
            menu_position: Point::ORIGIN,
            cursor_position: Point::ORIGIN,
        }
    }
}

/// A printer-list message.
#[derive(Clone, Debug)]
pub enum Message {
    /// Opens the Add Printer dialog.
    OpenAddPrinterDialog,
    /// Forwards an Add Printer dialog message.
    AddPrinter(crate::add_printer::Message),
    /// Selects a default-printer entry; index zero means no default.
    DefaultPrinterDropdown(usize),
    /// Reports a default-printer update.
    DefaultPrinterSet(Result<(), String>),
    /// Reloads printer state.
    Refresh,
    /// Reports loaded destinations and Printer Applications.
    PrintersLoaded(Result<PrintersLoad, String>),
    /// Reports refreshed Printer Applications.
    PrinterApplicationsLoaded(Result<Vec<PrinterApplication>, String>),
    /// Reports one changed printer from the backend cache.
    PrinterLoaded {
        /// Printer identifier from the event.
        printer_id: String,
        /// Updated printer, or `None` when it was removed.
        result: Result<Option<PrinterEntry>, String>,
    },
    /// Reports a printer's active-job count.
    JobsLoaded {
        /// Printer identifier.
        printer_id: String,
        /// Active-job count or loading error.
        result: Result<usize, String>,
    },
    /// Reports a backend cache change.
    PrintersEvent(PrintersEvent),
    /// Opens printer settings.
    OpenPrinterSettings(PrinterEntry),
    /// Opens a printer queue.
    OpenPrinterQueue(PrinterEntry),
    /// Sets a printer as the default.
    SetDefaultPrinter(String),
    /// Opens a printer or Printer Application web page.
    OpenPrinterWebPage(String),
    /// Reports whether the browser opened.
    PrinterWebPageOpened(Result<(), String>),
    /// Updates the pointer position.
    CursorMoved(Point),
    /// Opens a printer context menu.
    OpenPrinterMenu(String),
    /// Closes the context menu.
    CloseMenu,
    /// Forwards a popup-surface action.
    Surface(surface::Action),
}

impl State {
    fn default_printer_selection(&self) -> Option<usize> {
        let selected = self
            .default_printer_id
            .as_deref()
            .and_then(|default_id| {
                self.printers
                    .iter()
                    .position(|printer| printer.id() == default_id)
            })
            .map_or(0, |index| index + 1);

        Some(selected)
    }
}

impl State {
    /// Handles a printer-list message.
    pub fn update<M>(&mut self, message: Message) -> Task<M>
    where
        M: 'static
            + Send
            + From<Message>
            + From<crate::add_printer::Message>
            + From<crate::details::Message>
            + From<crate::queue::Message>
            + From<crate::details::Request>,
    {
        match message {
            Message::OpenAddPrinterDialog => self.open_add_printer_dialog(),
            Message::AddPrinter(message) => self.update_add_printer(message),
            Message::DefaultPrinterDropdown(index) => self.select_default_printer(index),
            Message::SetDefaultPrinter(printer_id) => {
                self.printer_context = None;
                self.default_printer_id = Some(printer_id.clone());
                set_default_printer_task(self.backend.clone(), printer_id)
            }
            Message::CursorMoved(position) => {
                self.cursor_position = position;
                Task::none()
            }
            Message::OpenPrinterMenu(printer_id) => {
                self.menu_position = self.cursor_position;
                self.printer_context = Some(printer_id);
                Task::none()
            }
            Message::CloseMenu => {
                self.printer_context = None;
                Task::none()
            }
            Message::DefaultPrinterSet(Ok(())) => Task::none(),
            Message::DefaultPrinterSet(Err(why)) => {
                tracing::warn!(why, "failed to set default printer");
                self.load_printers_task()
            }
            Message::Surface(action) => {
                cosmic::task::message(M::from(crate::details::Request::Surface(action)))
            }
            Message::Refresh => self.load_printers_task(),
            Message::PrintersLoaded(Ok(load)) => self.apply_printers_load(load),
            Message::PrintersLoaded(Err(why)) => {
                self.clear_printers_after_load_error(why);
                Task::none()
            }
            Message::PrinterApplicationsLoaded(Ok(applications)) => {
                self.printer_applications = applications;
                let backend = self.backend.clone();
                self.add_printer_task(move || crate::add_printer::State::refresh_task(backend))
            }
            Message::PrinterApplicationsLoaded(Err(why)) => {
                tracing::warn!(why, "failed to reload Printer Applications");
                Task::none()
            }
            Message::PrinterLoaded { printer_id, result } => {
                self.apply_printer_load(printer_id, result)
            }
            Message::JobsLoaded { printer_id, result } => {
                self.apply_active_job_count(printer_id, result);
                Task::none()
            }
            Message::PrintersEvent(event) => self.handle_printers_event(event),
            Message::OpenPrinterSettings(printer) => self.open_printer_settings(printer),
            Message::OpenPrinterQueue(printer) => self.open_printer_queue(printer),
            Message::OpenPrinterWebPage(web_page) => Self::open_printer_web_page(web_page),
            Message::PrinterWebPageOpened(result) => {
                if let Err(why) = result {
                    tracing::warn!(why, "failed to open printer web page");
                }
                Task::none()
            }
        }
    }

    fn open_add_printer_dialog<M>(&mut self) -> Task<M>
    where
        M: 'static + Send + From<crate::add_printer::Message>,
    {
        self.add_printer_dialog = Some(crate::add_printer::State::new(
            self.backend.clone(),
            self.printers.clone(),
        ));
        crate::add_printer::State::load_task(self.backend.clone())
    }

    fn select_default_printer<M>(&mut self, index: usize) -> Task<M>
    where
        M: 'static + Send + From<Message>,
    {
        let printer_id = index
            .checked_sub(1)
            .and_then(|printer_index| self.printers.get(printer_index))
            .map(|printer| printer.id().to_string());

        self.default_printer_id = printer_id.clone();

        match printer_id {
            Some(printer_id) => set_default_printer_task(self.backend.clone(), printer_id),
            None => clear_default_printer_task(self.backend.clone()),
        }
    }

    fn apply_printers_load<M>(&mut self, load: PrintersLoad) -> Task<M>
    where
        M: 'static + Send + From<Message> + From<crate::details::Message>,
    {
        self.default_printer_id = load
            .printers
            .iter()
            .find(|printer| printer.is_default())
            .map(|printer| printer.id().to_string());
        self.active_job_counts.retain(|printer_id, _| {
            load.printers
                .iter()
                .any(|printer| printer.id() == printer_id)
        });
        self.printers = load.printers;
        self.printer_applications = load.printer_applications;
        self.default_printer_labels = default_printer_labels(&self.printers);

        if let Some(dialog) = &mut self.add_printer_dialog {
            dialog.configured_printers = self.printers.clone();
        }

        // Refresh the details page's cached printer copy.
        Task::batch([
            self.load_active_jobs_task(),
            cosmic::task::message(M::from(crate::details::Message::PrintersRefreshed(
                self.printers.clone(),
            ))),
        ])
    }

    fn apply_printer_load<M>(
        &mut self,
        printer_id: String,
        result: Result<Option<PrinterEntry>, String>,
    ) -> Task<M>
    where
        M: 'static + Send + From<Message> + From<crate::details::Message>,
    {
        let printer = match result {
            Ok(printer) => printer,
            Err(why) => {
                tracing::warn!(printer_id, why, "failed to reload changed printer");
                return Task::none();
            }
        };

        if let Some(printer) = printer {
            let is_default = printer.is_default();

            if let Some(existing) = self
                .printers
                .iter_mut()
                .find(|existing| existing.id() == printer_id)
            {
                *existing = printer;
            } else {
                self.printers.push(printer);
                self.printers.sort_by(|left, right| left.id().cmp(right.id()));
            }

            if is_default {
                self.default_printer_id = Some(printer_id.clone());
                for printer in &mut self.printers {
                    printer.set_is_default(printer.id() == printer_id);
                }
            } else if self.default_printer_id.as_deref() == Some(printer_id.as_str()) {
                self.default_printer_id = None;
            }
        } else {
            self.printers.retain(|printer| printer.id() != printer_id);
            self.active_job_counts.remove(&printer_id);

            if self.default_printer_id.as_deref() == Some(printer_id.as_str()) {
                self.default_printer_id = None;
            }
            if self.printer_context.as_deref() == Some(printer_id.as_str()) {
                self.printer_context = None;
            }
        }

        self.default_printer_labels = default_printer_labels(&self.printers);

        if let Some(dialog) = &mut self.add_printer_dialog {
            dialog.configured_printers = self.printers.clone();
        }

        let details = cosmic::task::message(M::from(
            crate::details::Message::PrintersRefreshed(self.printers.clone()),
        ));

        if self.printers.iter().any(|printer| printer.id() == printer_id) {
            Task::batch([self.load_active_job_task(printer_id), details])
        } else {
            details
        }
    }

    fn clear_printers_after_load_error(&mut self, why: String) {
        tracing::error!(why, "failed to load printers");
        self.printers.clear();
        self.printer_applications.clear();
        self.default_printer_id = None;
        self.active_job_counts.clear();
        self.default_printer_labels = default_printer_labels(&self.printers);
    }

    fn apply_active_job_count(&mut self, printer_id: String, result: Result<usize, String>) {
        match result {
            Ok(count) => {
                self.active_job_counts.insert(printer_id, count);
            }
            // An unknown count must not appear as an empty queue.
            Err(why) => {
                tracing::warn!(printer_id, why, "failed to load active printer jobs");
            }
        }
    }

    fn handle_printers_event<M>(&self, event: PrintersEvent) -> Task<M>
    where
        M: 'static + Send + From<Message> + From<crate::add_printer::Message>,
    {
        match event.kind {
            PrintersEventKind::AvailableDestinationsChanged
            | PrintersEventKind::PrinterApplicationsChanged => Task::none(),
            PrintersEventKind::AddPrinterDiscoveryChanged => {
                let backend = self.backend.clone();
                self.add_printer_task(move || crate::add_printer::State::refresh_task(backend))
            }
            PrintersEventKind::PrinterConfigurationChanged => self.add_printer_task(|| {
                cosmic::task::message(M::from(crate::add_printer::Message::ConfigurationChanged))
            }),
        }
    }

    fn add_printer_task<M>(&self, task: impl FnOnce() -> Task<M>) -> Task<M> {
        if self.add_printer_dialog.is_some() {
            task()
        } else {
            Task::none()
        }
    }

    fn open_printer_settings<M>(&mut self, printer: PrinterEntry) -> Task<M>
    where
        M: 'static + Send + From<crate::details::Message> + From<crate::details::Request>,
    {
        self.printer_context = None;
        let is_default = self.default_printer_id.as_deref() == Some(printer.id());

        Task::batch([
            cosmic::task::message(M::from(crate::details::Message::LoadPrinter {
                printer,
                is_default,
                available_printers: self.printers.clone(),
            })),
            cosmic::task::message(M::from(crate::details::Request::ShowDetails)),
        ])
    }

    fn open_printer_queue<M>(&mut self, printer: PrinterEntry) -> Task<M>
    where
        M: 'static + Send + From<crate::queue::Message> + From<crate::details::Request>,
    {
        self.printer_context = None;

        Task::batch([
            cosmic::task::message(M::from(crate::queue::Message::LoadPrinter {
                printer: Box::new(printer),
                available_printers: self.printers.clone(),
            })),
            cosmic::task::message(M::from(crate::details::Request::ShowQueue)),
        ])
    }

    fn open_printer_web_page<M>(web_page: String) -> Task<M>
    where
        M: 'static + Send + From<Message>,
    {
        cosmic::task::future(async move {
            M::from(Message::PrinterWebPageOpened(
                crate::backend::open_printer_web_page(web_page).await,
            ))
        })
    }

    fn update_add_printer<M>(&mut self, message: crate::add_printer::Message) -> Task<M>
    where
        M: 'static + Send + From<Message> + From<crate::add_printer::Message>,
    {
        let Some(dialog) = &mut self.add_printer_dialog else {
            return Task::none();
        };

        match dialog.update(message) {
            crate::add_printer::Action::None => {}
            crate::add_printer::Action::Close => {
                self.add_printer_dialog = None;
            }
            crate::add_printer::Action::RefreshPrinters => {
                return self.load_printers_task();
            }
            // A fresh discovery round excludes the newly configured printer.
            crate::add_printer::Action::RediscoverPrinters => {
                return Task::batch([
                    self.load_printers_task(),
                    crate::add_printer::State::load_task(self.backend.clone()),
                ]);
            }
            crate::add_printer::Action::Task(task) => {
                return task;
            }
        }

        Task::none()
    }

    fn load_printers_task<M>(&self) -> Task<M>
    where
        M: 'static + Send + From<Message>,
    {
        let backend = self.backend.clone();

        cosmic::task::future(async move {
            M::from(Message::PrintersLoaded(load_printers(backend).await))
        })
    }

    fn load_active_job_task<M>(&self, printer_id: String) -> Task<M>
    where
        M: 'static + Send + From<Message>,
    {
        let backend = self.backend.clone();

        cosmic::task::future(async move {
            let result = load_active_job_count(backend, printer_id.clone()).await;
            M::from(Message::JobsLoaded { printer_id, result })
        })
    }

    fn load_active_jobs_task<M>(&self) -> Task<M>
    where
        M: 'static + Send + From<Message>,
    {
        Task::batch(
            self.printers
                .iter()
                .map(|printer| self.load_active_job_task(printer.id().to_string())),
        )
    }
}

/// Destinations and Printer Applications loaded as one grouping snapshot.
#[derive(Clone, Debug)]
pub struct PrintersLoad {
    printers: Vec<PrinterEntry>,
    printer_applications: Vec<PrinterApplication>,
}

async fn load_printers(backend: Backend) -> Result<PrintersLoad, String> {
    backend
        .refresh_available_destinations()
        .await
        .map_err(|why| why.to_string())?;
    backend
        .start_printer_application_discovery()
        .await
        .map_err(|why| why.to_string())?;
    let printers = backend.printers().await.map_err(|why| why.to_string())?;
    let printer_applications = backend
        .printer_applications()
        .await
        .map_err(|why| why.to_string())?;

    Ok(PrintersLoad {
        printers,
        printer_applications,
    })
}

/// Subscribes to printer events for the lifetime of the returned stream.
pub fn printer_events_subscription(backend: Backend) -> impl futures::Stream<Item = Message> {
    stream::channel(8, |tx: Sender<Message>| async move {
        std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };

            runtime.block_on(forward_printer_events(backend, tx));
        });

        future::pending::<()>().await;
    })
}

async fn forward_printer_events(backend: Backend, mut tx: Sender<Message>) {
    let (feed_tx, mut feed_rx) = futures::channel::mpsc::channel(8);
    let reading = backend.clone();
    let feeding = crate::backend::feed(backend, feed_tx);

    let forwarding = async move {
        while let Some(event) = feed_rx.next().await {
            let message = match event {
                // Reconnection can miss events, so recover once with a full snapshot.
                crate::backend::EventFeed::Reconnected => Message::Refresh,
                crate::backend::EventFeed::Changed(event) => match event.kind {
                    PrintersEventKind::AvailableDestinationsChanged => {
                        let Some(printer_id) = event.printer_id else {
                            if tx.send(Message::Refresh).await.is_err() {
                                return;
                            }
                            continue;
                        };
                        let result = reading
                            .printer(&printer_id)
                            .await
                            .map_err(|why| why.to_string());
                        Message::PrinterLoaded { printer_id, result }
                    }
                    PrintersEventKind::PrinterApplicationsChanged => {
                        let result = reading
                            .printer_applications()
                            .await
                            .map_err(|why| why.to_string());
                        Message::PrinterApplicationsLoaded(result)
                    }
                    _ => Message::PrintersEvent(event),
                },
            };

            if tx.send(message).await.is_err() {
                return;
            }
        }
    };

    futures::future::join(feeding, forwarding).await;
}

async fn load_active_job_count(backend: Backend, printer_id: String) -> Result<usize, String> {
    let jobs = backend
        .jobs(&printer_id, JobFilter::Active)
        .await
        .map_err(|why| why.to_string())?;

    Ok(jobs.len())
}

fn default_printer_labels(printers: &[PrinterEntry]) -> Vec<String> {
    std::iter::once(fl!("default-printer-not-set"))
        .chain(printers.iter().map(|printer| printer.name().to_string()))
        .collect()
}

/// Renders the default-printer selector.
pub fn default_printer_view<M: 'static + Clone>(
    state: &State,
    to_host: fn(Message) -> M,
) -> Element<'_, Message> {
    settings::section()
        .add(settings::item(
            fl!("default-printer"),
            default_printer_dropdown(state, to_host),
        ))
        .apply(Element::from)
}

/// Renders one row per grouped printer.
pub fn printers_view(state: &State) -> Element<'_, Message> {
    let groups = group_printers(state.printers.clone(), state.printer_applications.clone());

    if groups.is_empty() {
        return widget::list_column()
            .add(text::body(fl!("no-printers-found")))
            .apply(Element::from);
    }

    let mut groups_column = column::with_capacity(groups.len())
        .spacing(cosmic::theme::active().cosmic().space_xs())
        .width(Length::Fill);

    for group in groups {
        groups_column = groups_column.push(printer_group(state, group));
    }

    groups_column.apply(Element::from)
}

fn default_printer_dropdown<M: 'static + Clone>(
    state: &State,
    to_host: fn(Message) -> M,
) -> Element<'static, Message> {
    widget::dropdown::popup_dropdown(
        state.default_printer_labels.clone(),
        state.default_printer_selection(),
        Message::DefaultPrinterDropdown,
        cosmic::iced::window::Id::RESERVED,
        Message::Surface,
        to_host,
    )
    .into()
}

/// Renders the printer-page header.
pub fn page_header<'a>() -> Element<'a, Message> {
    row::with_capacity(2)
        .align_y(Alignment::Center)
        .spacing(cosmic::theme::active().cosmic().space_s())
        .push(text::title2(fl!("printers")).width(Length::Fill))
        .push(widget::button::standard(fl!("add-printer")).on_press(Message::OpenAddPrinterDialog))
        .apply(Element::from)
}

fn printer_group(state: &State, group: GroupedDestination) -> Element<'static, Message> {
    let mut card = widget::list_column()
        .divider_padding(0)
        .list_item_padding([0, 0]);

    if let Some(application) = group.printer_application() {
        card = card.add(printer_application_header(application));
    }

    for printer in group.queues() {
        card = card.add(printer_destination(state, printer));
    }

    card.apply(Element::from)
}

fn printer_application_header(application: &PrinterApplication) -> Element<'static, Message> {
    let spacing = cosmic::theme::active().cosmic().spacing;
    let title = text::heading(
        non_empty(&application.service_name)
            .map(str::to_owned)
            .unwrap_or_else(|| fl!("generic-printer-application")),
    )
    .width(Length::Fill)
    .wrapping(Wrapping::None)
    .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)));

    let mut header = row::with_capacity(2)
        .push(title)
        .align_y(Alignment::Center)
        .spacing(spacing.space_xxxs);

    if let Some(web_page) = printer_application_web_page(application) {
        header = header.push(icon_button(
            crate::icons::web_page(),
            Message::OpenPrinterWebPage(web_page),
        ));
    }

    header
        .padding([spacing.space_xxs, spacing.space_m])
        .width(Length::Fill)
        .apply(Element::from)
}

fn printer_destination(list: &State, printer: &PrinterEntry) -> Element<'static, Message> {
    let spacing = cosmic::theme::active().cosmic().spacing;
    let mut name_col = column::with_capacity(2).push(
        text::title4(printer.name().to_string())
            .wrapping(Wrapping::None)
            .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1))),
    );

    if let Some(subtitle) = printer_subtitle(printer) {
        name_col = name_col.push(
            text::caption(subtitle)
                .wrapping(Wrapping::None)
                .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1))),
        );
    }

    let state = presentation(list, printer);
    let status_row = row::with_capacity(2)
        .push(crate::widgets::dot(state.color, 8.0))
        .push(text::body(state.status))
        .spacing(spacing.space_xxxs)
        .align_y(Alignment::Center);

    let information = column::with_capacity(2)
        .push(name_col)
        .push(
            column::with_capacity(2)
                .push(status_row)
                .push(text::body(state.detail)),
        )
        .spacing(spacing.space_xxs)
        .width(Length::Fill);

    let destination = column::with_capacity(2)
        .push(information)
        .push(printer_destination_actions(printer))
        .spacing(spacing.space_s)
        .padding([spacing.space_s, spacing.space_m])
        .width(Length::Fill);

    let trigger = widget::mouse_area(destination)
        .on_move(Message::CursorMoved)
        .on_right_press(Message::OpenPrinterMenu(printer.id().to_string()));

    if list.printer_context.as_deref() != Some(printer.id()) {
        return trigger.into();
    }

    // `context_menu` offsets from the full row width instead of the pointer.
    widget::popover(trigger)
        .position(widget::popover::Position::Point(list.menu_position))
        .popup(printer_context_menu(list, printer))
        .on_close(Message::CloseMenu)
        .into()
}

fn printer_destination_actions(printer: &PrinterEntry) -> Element<'static, Message> {
    let spacing = cosmic::theme::active().cosmic().spacing;
    let mut left = row::with_capacity(2)
        .spacing(spacing.space_xxxs)
        .align_y(Alignment::Center);

    if let Some(web_page) = printer.web_page() {
        left = left.push(icon_button(
            crate::icons::web_page(),
            Message::OpenPrinterWebPage(web_page.to_string()),
        ));
    }
    left = left.push(icon_button(
        crate::icons::printer_queue(),
        Message::OpenPrinterQueue(printer.clone()),
    ));

    row::with_capacity(2)
        .push(left.width(Length::Fill))
        .push(settings_link(printer))
        .align_y(Alignment::Center)
        .spacing(spacing.space_xxs)
        .width(Length::Fill)
        .apply(Element::from)
}

fn settings_link(printer: &PrinterEntry) -> Element<'static, Message> {
    widget::button::custom(
        row::with_capacity(2)
            .align_y(Alignment::Center)
            .spacing(cosmic::theme::active().cosmic().space_xxxs())
            .push(text::body(fl!("settings")))
            .push(widget::icon::from_name("go-next-symbolic").size(crate::style::ICON_SIZE)),
    )
    .class(cosmic::theme::Button::Link)
    .on_press(Message::OpenPrinterSettings(printer.clone()))
    .into()
}

fn icon_button(name: &'static str, message: Message) -> Element<'static, Message> {
    widget::button::icon(widget::icon::from_name(name))
        .on_press(message)
        .into()
}

fn printer_context_menu(state: &State, printer: &PrinterEntry) -> Element<'static, Message> {
    let is_default = state.default_printer_id.as_deref() == Some(printer.id());
    let rows = [
        (
            fl!("set-as-default-printer"),
            (!is_default).then(|| Message::SetDefaultPrinter(printer.id().to_string())),
        ),
        (
            fl!("printer-queue"),
            Some(Message::OpenPrinterQueue(printer.clone())),
        ),
        (
            fl!("printer-settings"),
            Some(Message::OpenPrinterSettings(printer.clone())),
        ),
        (
            fl!("printer-web-interface"),
            printer
                .web_page()
                .map(|web_page| Message::OpenPrinterWebPage(web_page.to_string())),
        ),
    ];

    let spacing = cosmic::theme::active().cosmic().spacing;
    let mut menu = column::with_capacity(rows.len().saturating_mul(2));

    for (index, (label, message)) in rows.into_iter().enumerate() {
        if index > 0 {
            menu = menu.push(
                container(widget::divider::horizontal::light()).padding([0, spacing.space_xxs]),
            );
        }
        menu = menu.push(context_menu_row(label, message));
    }

    container(menu)
        .padding([spacing.space_xxs, 0])
        .width(Length::Fixed(CONTEXT_MENU_WIDTH))
        .class(cosmic::theme::Container::Dropdown)
        .into()
}

fn context_menu_row(label: String, message: Option<Message>) -> Element<'static, Message> {
    menu::menu_button(vec![text::body(label).width(Length::Fill).into()])
        .height(Length::Fixed(CONTEXT_MENU_ROW_HEIGHT))
        .on_press_maybe(message)
        .into()
}

fn printer_subtitle(printer: &PrinterEntry) -> Option<String> {
    printer
        .model()
        .and_then(non_empty)
        .filter(|model| *model != printer.name())
        .map(str::to_owned)
}

fn active_job_count(state: &State, printer: &PrinterEntry) -> usize {
    state
        .active_job_counts
        .get(printer.id())
        .copied()
        .unwrap_or_default()
}

struct Presentation {
    status: String,
    color: Color,
    detail: String,
}

// Attention states outrank queued work and informational reasons.
fn presentation(state: &State, printer: &PrinterEntry) -> Presentation {
    let (status, color) = match printer.status() {
        PrinterStatus::Ready => (fl!("printer-ready"), crate::style::status_ready()),
        PrinterStatus::Offline => (fl!("printer-stopped"), crate::style::status_stopped()),
        PrinterStatus::LowToner => (fl!("printer-low-toner"), crate::style::status_stopped()),
    };
    let reason = crate::state_reason::worst(printer);

    if let Some(reason) = &reason
        && reason.severity > crate::state_reason::Severity::Report
    {
        return Presentation {
            status,
            color,
            detail: reason.text.clone(),
        };
    }

    let waiting = active_job_count(state, printer);

    if waiting > 0 {
        return Presentation {
            status: fl!("printer-printing"),
            color: crate::style::status_printing(),
            detail: documents_waiting(waiting),
        };
    }

    Presentation {
        status,
        color,
        detail: reason.map_or_else(|| fl!("no-jobs-waiting"), |reason| reason.text),
    }
}

fn documents_waiting(count: usize) -> String {
    if count == 1 {
        fl!("one-document")
    } else {
        fl!("documents-count", count = count)
    }
}

fn printer_application_web_page(application: &PrinterApplication) -> Option<String> {
    application
        .txt
        .get("adminurl")
        .and_then(|url| non_empty(url))
        .map(str::to_owned)
        .or_else(|| {
            let (scheme, rest) = application.system_uri.split_once("://")?;
            let authority = rest.split('/').next()?;
            let web_scheme = match scheme {
                "ipp" => "http",
                "ipps" => "https",
                _ => return None,
            };

            Some(format!("{web_scheme}://{authority}/"))
        })
}

fn set_default_printer_task<M>(backend: Backend, printer_id: String) -> Task<M>
where
    M: 'static + Send + From<Message>,
{
    cosmic::task::future(async move {
        M::from(Message::DefaultPrinterSet(
            backend
                .set_printer_default(&printer_id)
                .await
                .map_err(|why| why.to_string()),
        ))
    })
}

fn clear_default_printer_task<M>(backend: Backend) -> Task<M>
where
    M: 'static + Send + From<Message>,
{
    cosmic::task::future(async move {
        M::from(Message::DefaultPrinterSet(
            backend
                .clear_printer_default()
                .await
                .map_err(|why| why.to_string()),
        ))
    })
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}
