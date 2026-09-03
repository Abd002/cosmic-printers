use std::rc::Rc;

use crate::backend::{Backend, BackendError};
use cosmic::app::Task;
use cosmic::iced::core::text::{Ellipsize, EllipsizeHeightLimit, Wrapping};
use cosmic::iced::widget::scrollable::{Direction, Scrollbar};
use cosmic::iced::{Alignment, Length, Padding, Size, window};
use cosmic::widget::{
    self, button, column, container, icon, row, scrollable,
    space::{horizontal as horizontal_space, vertical as vertical_space},
    text,
};
use cosmic::{Apply, Element, cosmic_theme};
use cosmic_settings_printers_core::{
    AddPrinterDiscoveryReply, AddPrinterDiscoveryState, ConfigureDiscoveredPrinterRequest,
    ConfigurePrinterReply, DiscoveredPhysicalPrinter, DiscoveryGeneration, Error as PrinterError,
    ManualSetupPrinterApplication, PaCandidateState, PrinterApplicationCandidateSummary,
    PrinterConfigurationState, PrinterEntry,
};

const INITIAL_WINDOW_SIZE: Size = Size::new(680.0, 570.0);
const SEARCH_MAX_WIDTH: f32 = 314.0;
const PRINTER_ROW_HEIGHT: f32 = 54.0;
const APPLICATION_ROW_HEIGHT: f32 = 48.0;

/// Active Add Printer dialog view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogView {
    /// Displays discovered printers.
    Discovery,
    /// Selects a Printer Application.
    SelectApplication {
        /// Printer awaiting configuration.
        printer_id: String,
    },
    /// Displays manual setup options.
    ManualSetup,
    /// Displays configuration progress.
    Adding {
        /// Printer being configured.
        printer_id: String,
    },
    /// Displays configuration completion.
    Added,
}

/// State for the Add Printer dialog.
#[derive(Clone, Debug)]
pub struct State {
    backend: Backend,
    window_id: Option<window::Id>,
    /// Search query.
    pub search: String,
    /// Current user-visible error.
    pub error: Option<String>,
    /// Existing configured printers.
    pub configured_printers: Vec<PrinterEntry>,
    /// Active dialog view.
    pub view: DialogView,
    discovery: Option<AddPrinterDiscoveryReply>,
    manual_setup_applications: Vec<ManualSetupPrinterApplication>,
    pending_operation: Option<String>,
    added: Vec<AddedPrinter>,
}

#[derive(Clone, Debug)]
struct AddedPrinter {
    name: String,
    destination_id: Option<String>,
}

/// Snapshot of an Add Printer discovery round.
#[derive(Clone, Debug)]
pub struct Load {
    discovery: AddPrinterDiscoveryReply,
    manual_setup_applications: Vec<ManualSetupPrinterApplication>,
}

impl State {
    /// Creates dialog state with the configured printers.
    #[must_use]
    pub fn new(backend: Backend, configured_printers: Vec<PrinterEntry>) -> Self {
        Self {
            backend,
            window_id: None,
            search: String::new(),
            error: None,
            configured_printers,
            view: DialogView::Discovery,
            discovery: None,
            manual_setup_applications: Vec::new(),
            pending_operation: None,
            added: Vec::new(),
        }
    }

    /// Sets the printer backend.
    pub fn set_backend(&mut self, backend: Backend) {
        self.backend = backend;
    }

    pub(crate) fn set_window_id(&mut self, window_id: window::Id) {
        self.window_id = Some(window_id);
    }

    pub(crate) fn window_id(&self) -> Option<window::Id> {
        self.window_id
    }

    /// Starts and loads a discovery round.
    pub fn load_task<M>(backend: Backend) -> Task<M>
    where
        M: 'static + Send + From<Message>,
    {
        loaded_task(start_discovery(backend))
    }

    /// Refreshes the active discovery round.
    pub fn refresh_task<M>(backend: Backend) -> Task<M>
    where
        M: 'static + Send + From<Message>,
    {
        loaded_task(load(backend))
    }

    /// Handles an Add Printer message.
    pub fn update<M>(&mut self, message: Message) -> Action<M>
    where
        M: 'static + Send + From<Message>,
    {
        match message {
            Message::Close => Action::Close,
            Message::DragWindow => self.window_id.map_or(Action::None, |window_id| {
                Action::Task(window::drag::<cosmic::Action<M>>(window_id))
            }),
            Message::ToggleMaximizeWindow => self.window_id.map_or(Action::None, |window_id| {
                Action::Task(window::toggle_maximize::<cosmic::Action<M>>(window_id))
            }),
            Message::Search(search) => {
                self.search = search;
                Action::None
            }
            Message::Loaded(result) => {
                self.apply_load(result);
                Action::None
            }
            Message::ConfigurationChanged => self.poll_configuration(),
            Message::OpenManualSetup => {
                self.open_manual_setup();
                Action::None
            }
            Message::SelectDiscoveredPrinter(printer_id) => {
                self.select_discovered_printer(printer_id)
            }
            Message::SelectPrinterApplication(candidate_id) => {
                self.select_printer_application(candidate_id)
            }
            Message::OpenPrinterWebPage(web_page) => Self::open_web_page(web_page),
            Message::PrinterSetupFinished(result) => self.finish_printer_setup(result),
            Message::WebPageOpened(result) => self.finish_web_page_open(result),
        }
    }

    // An empty early round does not mean Printer Application discovery is finished.
    fn apply_load(&mut self, result: Result<Load, String>) {
        match result {
            Ok(load) => {
                self.error = None;
                self.discovery = Some(load.discovery);
                self.manual_setup_applications = load.manual_setup_applications;
            }
            Err(why) => {
                tracing::error!(why, "failed to load add printer discovery");
                self.error = Some(fl!("failed-to-load-printers"));
            }
        }
    }

    fn open_manual_setup(&mut self) {
        if self.is_adding() {
            return;
        }

        self.error = None;
        self.view = DialogView::ManualSetup;
    }

    fn select_discovered_printer<M>(&mut self, printer_id: String) -> Action<M>
    where
        M: 'static + Send + From<Message>,
    {
        if self.is_searching() || self.is_adding() {
            return Action::None;
        }

        let Some(printer) = self.physical_printer(&printer_id) else {
            self.error = Some(fl!("no-printers-found"));
            return Action::None;
        };

        match selectable_candidate_ids(printer).as_slice() {
            [] => {
                self.open_manual_setup();
                Action::None
            }
            [candidate_id] => {
                let candidate_id = candidate_id.clone();
                self.start_setup(printer_id, candidate_id)
            }
            _ => {
                self.error = None;
                self.view = DialogView::SelectApplication { printer_id };
                Action::None
            }
        }
    }

    fn select_printer_application<M>(&mut self, candidate_id: String) -> Action<M>
    where
        M: 'static + Send + From<Message>,
    {
        let DialogView::SelectApplication { printer_id } = &self.view else {
            return Action::None;
        };
        let printer_id = printer_id.clone();

        let offered = self
            .physical_printer(&printer_id)
            .is_some_and(|printer| selectable_candidate_ids(printer).contains(&candidate_id));

        if offered {
            self.start_setup(printer_id, candidate_id)
        } else {
            Action::None
        }
    }

    fn start_setup<M>(&mut self, printer_id: String, candidate_id: String) -> Action<M>
    where
        M: 'static + Send + From<Message>,
    {
        if self.is_adding() {
            return Action::None;
        }

        // Do not configure candidates from a stale discovery generation.
        let Some(discovery_generation) = self.selectable_generation() else {
            self.view = DialogView::Discovery;
            return Action::Task(Self::load_task(self.backend.clone()));
        };

        self.error = None;
        self.view = DialogView::Adding {
            printer_id: printer_id.clone(),
        };

        Action::Task(setup_task(
            self.backend.clone(),
            ConfigureDiscoveredPrinterRequest {
                discovery_generation,
                physical_printer_id: printer_id,
                candidate_id,
                requested_display_name: None,
            },
        ))
    }

    fn poll_configuration<M>(&self) -> Action<M>
    where
        M: 'static + Send + From<Message>,
    {
        let Some(operation_id) = self.pending_operation.clone() else {
            return Action::None;
        };

        let backend = self.backend.clone();

        Action::Task(cosmic::task::future(async move {
            M::from(Message::PrinterSetupFinished(
                printer_configuration(backend, operation_id).await,
            ))
        }))
    }

    fn finish_printer_setup<M>(
        &mut self,
        result: Result<ConfigurePrinterReply, SetupError>,
    ) -> Action<M>
    where
        M: 'static + Send + From<Message>,
    {
        self.pending_operation = None;

        match result {
            Ok(reply) => self.apply_configuration(reply),
            Err(SetupError::ManualSetup { web_interface_uri }) => {
                self.error = None;
                self.continue_in_printer_application(web_interface_uri)
            }
            Err(SetupError::Failed(why)) => {
                tracing::error!(why, "failed to configure discovered printer");
                self.error = Some(why);
                self.view = DialogView::Discovery;
                Action::None
            }
        }
    }

    fn apply_configuration<M>(&mut self, reply: ConfigurePrinterReply) -> Action<M>
    where
        M: 'static + Send + From<Message>,
    {
        match reply.state {
            PrinterConfigurationState::Creating => {
                self.pending_operation = Some(reply.operation_id);
                Action::RefreshPrinters
            }
            PrinterConfigurationState::AwaitingAdvertisement
            | PrinterConfigurationState::Reconciled
            | PrinterConfigurationState::AlreadyConfigured => {
                self.error = None;
                self.added.push(AddedPrinter {
                    name: reply.configured_printer_name,
                    destination_id: reply.destination_id,
                });
                self.view = DialogView::Added;
                Action::RediscoverPrinters
            }
            PrinterConfigurationState::ManualActionRequired => {
                self.continue_in_printer_application(reply.web_interface_uri)
            }
            PrinterConfigurationState::UnknownOutcome | PrinterConfigurationState::Failed => {
                self.error = Some(fl!("failed-to-add-printer"));
                self.view = DialogView::Discovery;
                Action::None
            }
        }
    }

    fn continue_in_printer_application<M>(&mut self, web_interface_uri: Option<String>) -> Action<M>
    where
        M: 'static + Send + From<Message>,
    {
        self.view = DialogView::ManualSetup;

        match web_interface_uri {
            Some(web_page) => Self::open_web_page(web_page),
            None => {
                self.error = Some(fl!("printer-application-web-interface-unavailable"));
                Action::None
            }
        }
    }

    // Preserve the URL when no desktop handler can open it.
    fn finish_web_page_open<M>(&mut self, result: Result<(), (String, String)>) -> Action<M> {
        if let Err((address, why)) = result {
            tracing::error!(why, address, "failed to open printer web page");
            self.error = Some(fl!(
                "open-printer-application-page-manually",
                address = address
            ));
        }

        Action::None
    }

    fn visible_printers(&self) -> impl Iterator<Item = &DiscoveredPhysicalPrinter> {
        let search = self.search.trim().to_lowercase();

        self.printers_to_set_up()
            .filter(move |printer| printer_matches_search(printer, &search))
    }

    fn printers_to_set_up(&self) -> impl Iterator<Item = &DiscoveredPhysicalPrinter> {
        self.physical_printers()
            .iter()
            .filter(|printer| !is_configured(printer))
    }

    fn physical_printers(&self) -> &[DiscoveredPhysicalPrinter] {
        self.discovery
            .as_ref()
            .map_or(&[], |discovery| discovery.physical_printers.as_slice())
    }

    fn physical_printer(&self, printer_id: &str) -> Option<&DiscoveredPhysicalPrinter> {
        self.physical_printers()
            .iter()
            .find(|printer| printer.id == printer_id)
    }

    fn configured_printer(&self, destination_id: &str) -> Option<&PrinterEntry> {
        self.configured_printers
            .iter()
            .find(|printer| printer.id() == destination_id)
    }

    fn selectable_generation(&self) -> Option<DiscoveryGeneration> {
        self.discovery
            .as_ref()
            .filter(|discovery| !discovery.cached)
            .map(|discovery| discovery.generation)
    }

    fn every_application_answered(&self) -> bool {
        self.discovery.as_ref().is_some_and(|discovery| {
            discovery.completed_printer_application_scans
                >= discovery.total_printer_application_scans
        })
    }

    fn is_searching(&self) -> bool {
        self.error.is_none()
            && self
                .discovery
                .as_ref()
                .is_none_or(|discovery| discovery.state == AddPrinterDiscoveryState::Searching)
    }

    fn is_adding(&self) -> bool {
        matches!(self.view, DialogView::Adding { .. })
    }

    fn open_web_page<M>(web_page: String) -> Action<M>
    where
        M: 'static + Send + From<Message>,
    {
        Action::Task(cosmic::task::future(async move {
            let opened = crate::backend::open_printer_web_page(web_page.clone())
                .await
                .map_err(|why| (web_page, why));

            M::from(Message::WebPageOpened(opened))
        }))
    }
}

pub(crate) fn open_window<M>(application_id: &str) -> (window::Id, Task<M>)
where
    M: 'static + Send,
{
    let mut settings = window::Settings {
        decorations: false,
        min_size: Some(Size::new(360.0, 180.0)),
        resizable: true,
        size: INITIAL_WINDOW_SIZE,
        transparent: true,
        ..Default::default()
    };

    #[cfg(target_os = "linux")]
    {
        settings.platform_specific.application_id = application_id.to_string();
    }

    let (window_id, task) = window::open(settings);
    (window_id, task.map(|_| cosmic::action::none()))
}

/// Messages handled by the Add Printer dialog.
#[derive(Clone, Debug)]
pub enum Message {
    /// Closes the dialog.
    Close,
    /// Starts moving the dialog window.
    DragWindow,
    /// Toggles the dialog window between maximized and restored.
    ToggleMaximizeWindow,
    /// Updates the search query.
    Search(String),
    /// Reports discovery results.
    Loaded(Result<Load, String>),
    /// Refreshes configuration progress.
    ConfigurationChanged,
    /// Opens manual setup.
    OpenManualSetup,
    /// Selects a discovered printer.
    SelectDiscoveredPrinter(String),
    /// Selects a Printer Application.
    SelectPrinterApplication(String),
    /// Opens a Printer Application web interface.
    OpenPrinterWebPage(String),
    /// Reports configuration completion.
    PrinterSetupFinished(Result<ConfigurePrinterReply, SetupError>),
    /// Reports whether a web interface opened.
    WebPageOpened(Result<(), (String, String)>),
}

/// Errors from configuring a discovered printer.
#[derive(Clone, Debug)]
pub enum SetupError {
    /// Requires setup through the Printer Application.
    ManualSetup {
        /// Optional setup page.
        web_interface_uri: Option<String>,
    },
    /// Configuration failed.
    Failed(String),
}

impl From<Message> for crate::list::Message {
    fn from(message: Message) -> Self {
        crate::list::Message::AddPrinter(message)
    }
}

/// Actions returned to the dialog host.
pub enum Action<M> {
    /// No action.
    None,
    /// Closes the dialog.
    Close,
    /// Refreshes configured printers.
    RefreshPrinters,
    /// Refreshes printers and restarts discovery.
    RediscoverPrinters,
    /// Runs a dialog task.
    Task(Task<M>),
}

fn loaded_task<M>(load: impl Future<Output = Result<Load, String>> + Send + 'static) -> Task<M>
where
    M: 'static + Send + From<Message>,
{
    cosmic::task::future(async move { M::from(Message::Loaded(load.await)) })
}

fn setup_task<M>(backend: Backend, request: ConfigureDiscoveredPrinterRequest) -> Task<M>
where
    M: 'static + Send + From<Message>,
{
    cosmic::task::future(async move {
        M::from(Message::PrinterSetupFinished(
            configure(backend, request).await,
        ))
    })
}

async fn start_discovery(backend: Backend) -> Result<Load, String> {
    backend
        .start_add_printer_discovery()
        .await
        .map_err(|why| why.to_string())?;

    discovery_load(&backend).await
}

async fn load(backend: Backend) -> Result<Load, String> {
    discovery_load(&backend).await
}

async fn discovery_load(backend: &Backend) -> Result<Load, String> {
    let discovery = backend
        .add_printer_discovery()
        .await
        .map_err(|why| why.to_string())?;
    let manual_setup_applications = backend
        .manual_setup_printer_applications()
        .await
        .map_err(|why| why.to_string())?;

    Ok(Load {
        discovery,
        manual_setup_applications,
    })
}

async fn configure(
    backend: Backend,
    request: ConfigureDiscoveredPrinterRequest,
) -> Result<ConfigurePrinterReply, SetupError> {
    backend
        .configure_discovered_printer(request)
        .await
        .map_err(setup_error)
}

async fn printer_configuration(
    backend: Backend,
    operation_id: String,
) -> Result<ConfigurePrinterReply, SetupError> {
    backend
        .printer_configuration(&operation_id)
        .await
        .map_err(setup_error)
}

fn setup_error(error: BackendError) -> SetupError {
    match error {
        BackendError::Service(PrinterError::PrinterConfigurationManualActionRequired {
            web_interface_uri,
            ..
        }) => SetupError::ManualSetup { web_interface_uri },
        error => SetupError::Failed(error.to_string()),
    }
}

/// Returns the active Add Printer dialog view.
pub fn dialog(state: &State) -> Element<'_, Message> {
    let spacing = cosmic::theme::spacing();
    let body = match &state.view {
        DialogView::Discovery | DialogView::Adding { .. } => discovery_view(state),
        DialogView::ManualSetup => manual_setup_view(state),
        DialogView::SelectApplication { printer_id } => select_application_view(state, printer_id),
        DialogView::Added => added_printers_view(state),
    };

    let body_padding = match state.view {
        DialogView::ManualSetup | DialogView::SelectApplication { .. } => Padding::ZERO
            .top(spacing.space_l)
            .bottom(spacing.space_xxl)
            .horizontal(spacing.space_xxl),
        _ => Padding::ZERO
            .bottom(spacing.space_l)
            .horizontal(spacing.space_xxl),
    };
    let drag_region = widget::mouse_area(
        vertical_space()
            .height(Length::Fixed(f32::from(spacing.space_l)))
            .width(Length::Fill),
    )
    .on_press(Message::DragWindow)
    .on_double_click(Message::ToggleMaximizeWindow);

    widget::layer_container(
        column::with_capacity(3)
            .push(drag_region)
            .push(
                scrollable(body)
                    .direction(Direction::Vertical(Scrollbar::hidden()))
                    .apply(container)
                    .padding(body_padding)
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .push(dialog_footer(state))
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .layer(cosmic_theme::Layer::Background)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn dialog_footer(state: &State) -> Element<'static, Message> {
    let cosmic_theme::Spacing {
        space_xs,
        space_xxs,
        ..
    } = cosmic::theme::spacing();
    let label = match state.view {
        DialogView::Added | DialogView::ManualSetup => fl!("close"),
        _ => fl!("cancel"),
    };

    container(
        widget::layer_container(
            row::with_capacity(2)
                .align_y(Alignment::Center)
                .spacing(space_xxs)
                .push(horizontal_space())
                .push(widget::button::standard(label).on_press(Message::Close))
                .width(Length::Fill),
        )
        .layer(cosmic_theme::Layer::Primary)
        .padding([space_xxs, space_xs])
        .width(Length::Fill),
    )
    .padding(space_xxs)
    .width(Length::Fill)
    .into()
}

fn discovery_view(state: &State) -> Element<'_, Message> {
    let spacing = cosmic::theme::active().cosmic().spacing;
    let search = container(
        container(
            widget::search_input(fl!("type-to-search"), &state.search)
                .on_input(Message::Search)
                .on_clear(Message::Search(String::new()))
                .width(Length::Fill),
        )
        .max_width(SEARCH_MAX_WIDTH)
        .width(Length::Fill),
    )
    .width(Length::Fill)
    .center_x(Length::Fill);

    let mut settings = column::with_capacity(3)
        .spacing(spacing.space_s)
        .push(search)
        .push(printers_section(state));
    if !state.is_searching() {
        settings = settings.push(manual_setup_prompt());
    }

    settings.width(Length::Fill).into()
}

fn manual_setup_view(state: &State) -> Element<'_, Message> {
    let mut rows = Vec::with_capacity(state.manual_setup_applications.len().max(1) + 1);
    if let Some(error) = &state.error {
        rows.push(plain_row(error.clone()));
    }
    rows.extend(
        state
            .manual_setup_applications
            .iter()
            .map(manual_application_row),
    );
    if rows.is_empty() {
        rows.push(plain_row(fl!("no-printer-applications-found")));
    }
    let rows = with_dividers(rows);

    let spacing = cosmic::theme::active().cosmic().spacing;

    column::with_capacity(2)
        .spacing(spacing.space_m)
        .push(section_heading(fl!(
            "use-a-printer-application-to-manually-set-up-a-printer"
        )))
        .push(application_list(rows))
        .width(Length::Fill)
        .into()
}

fn select_application_view<'a>(state: &'a State, printer_id: &str) -> Element<'a, Message> {
    let rows = state
        .physical_printer(printer_id)
        .map(|printer| {
            with_dividers(
                printer
                    .candidates
                    .iter()
                    .filter(|candidate| is_selectable(candidate.state))
                    .map(select_application_row)
                    .collect(),
            )
        })
        .unwrap_or_default();

    let spacing = cosmic::theme::active().cosmic().spacing;

    column::with_capacity(2)
        .spacing(spacing.space_m)
        .push(section_heading(fl!(
            "choose-the-printer-application-to-set-up-your-printer"
        )))
        .push(application_list(if rows.is_empty() {
            vec![plain_row(fl!("no-printer-applications-found"))]
        } else {
            rows
        }))
        .width(Length::Fill)
        .into()
}

fn added_printers_view(state: &State) -> Element<'_, Message> {
    let rows = with_dividers(
        state
            .added
            .iter()
            .map(|added| added_printer_row(state, added))
            .collect(),
    );
    let spacing = cosmic::theme::active().cosmic().spacing;
    let description = row::with_capacity(2)
        .spacing(spacing.space_xxxs)
        .align_y(Alignment::Center)
        .push(text::body(fl!("printer-web-interface-description")).width(Length::Fill))
        .push(
            icon::from_name(crate::icons::web_page())
                .size(crate::style::ICON_SIZE)
                .icon()
                .class(cosmic::theme::Svg::Custom(primary_svg())),
        );

    column::with_capacity(2)
        .spacing(spacing.space_s)
        .push(
            column::with_capacity(2)
                .spacing(spacing.space_xxs)
                .push(section_heading(fl!("added-printers")))
                .push(list_view(rows)),
        )
        .push(description)
        .width(Length::Fill)
        .into()
}

fn printers_section(state: &State) -> Element<'_, Message> {
    let rows = if state.is_searching() {
        vec![plain_row(fl!("searching"))]
    } else if let Some(error) = &state.error {
        vec![plain_row(error.clone())]
    } else {
        let printers = state.visible_printers().collect::<Vec<_>>();
        if printers.is_empty() {
            vec![plain_row(fl!("no-printers-found"))]
        } else {
            with_dividers(
                printers
                    .iter()
                    .map(|printer| discovered_printer_row(state, printer))
                    .collect(),
            )
        }
    };
    let spacing = cosmic::theme::active().cosmic().spacing;

    column::with_capacity(2)
        .spacing(spacing.space_xxs)
        .push(section_heading(fl!("printers")))
        .push(list_view(rows))
        .into()
}

fn manual_setup_prompt() -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();
    let trailing_size = f32::from(spacing.space_l);
    let chevron: Element<'static, Message> = container(
        icon::from_name("go-next-symbolic")
            .size(crate::style::ICON_SIZE)
            .icon()
            .class(cosmic::theme::Svg::Custom(primary_svg())),
    )
    .width(Length::Fixed(trailing_size))
    .height(Length::Fixed(trailing_size))
    .center(Length::Fixed(trailing_size))
    .into();

    column::with_capacity(2)
        .spacing(spacing.space_xxs)
        .push(regular_heading(fl!("your-printer-not-discovered")))
        .push(row_button(
            row::with_capacity(2)
                .align_y(Alignment::Center)
                .spacing(spacing.space_s)
                .push(row_label(fl!("manual-setup")))
                .push(chevron),
            APPLICATION_ROW_HEIGHT,
            Some(Message::OpenManualSetup),
        ))
        .into()
}

fn discovered_printer_row(
    state: &State,
    printer: &DiscoveredPhysicalPrinter,
) -> Element<'static, Message> {
    let printer_id = printer.id.clone();
    let connecting = matches!(
        &state.view,
        DialogView::Adding { printer_id: adding_id } if adding_id == &printer_id
    );
    let status = if connecting {
        fl!("connecting")
    } else {
        candidate_summary(printer, state.every_application_answered())
    };
    let content = two_line_printer_content(printer.display_name.clone(), status, connecting, None);

    row_button(
        content,
        PRINTER_ROW_HEIGHT,
        (!state.is_adding()).then_some(Message::SelectDiscoveredPrinter(printer_id)),
    )
}

fn added_printer_row(state: &State, added: &AddedPrinter) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();
    let destination = added
        .destination_id
        .as_deref()
        .and_then(|destination_id| state.configured_printer(destination_id));
    let name = destination.map_or_else(|| added.name.clone(), printer_display_name);
    let trailing = destination
        .and_then(PrinterEntry::web_page)
        .map(|web_page| {
            widget::button::icon(icon::from_name(crate::icons::web_page()))
                .on_press(Message::OpenPrinterWebPage(web_page.to_string()))
                .into()
        });

    container(two_line_printer_content(
        name,
        fl!("printer-ready"),
        true,
        trailing,
    ))
    .padding([spacing.space_xxs, spacing.space_m])
    .width(Length::Fill)
    .height(Length::Fixed(PRINTER_ROW_HEIGHT))
    .into()
}

fn two_line_printer_content(
    name: String,
    caption: String,
    checked: bool,
    trailing: Option<Element<'static, Message>>,
) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();
    let check: Element<'static, Message> = if checked {
        icon::from_name("checkbox-checked-symbolic")
            .size(crate::style::ICON_SIZE)
            .icon()
            .class(cosmic::theme::Svg::Custom(accent_svg()))
            .into()
    } else {
        horizontal_space()
            .width(Length::Fixed(f32::from(crate::style::ICON_SIZE)))
            .into()
    };
    let copy = column::with_capacity(2)
        .spacing(0)
        .push(row_label(name))
        .push(
            text::caption(caption)
                .wrapping(Wrapping::None)
                .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)))
                .width(Length::Fill),
        )
        .width(Length::Fill);
    let left = row::with_capacity(2)
        .align_y(Alignment::Center)
        .spacing(spacing.space_xxs)
        .push(check)
        .push(copy)
        .width(Length::Fill);
    let mut content = row::with_capacity(2)
        .align_y(Alignment::Center)
        .spacing(spacing.space_s)
        .push(left);
    if let Some(trailing) = trailing {
        content = content.push(trailing);
    }

    content.width(Length::Fill).into()
}

fn manual_application_row(
    application: &ManualSetupPrinterApplication,
) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();

    row::with_capacity(2)
        .align_y(Alignment::Center)
        .spacing(spacing.space_s)
        .push(row_label(application_display_name(application)))
        .push(
            widget::button::text(fl!("set-up-printer")).on_press(Message::OpenPrinterWebPage(
                application.web_interface_uri.clone(),
            )),
        )
        .padding([spacing.space_xxs, spacing.space_m])
        .width(Length::Fill)
        .height(Length::Fixed(APPLICATION_ROW_HEIGHT))
        .into()
}

fn select_application_row(
    candidate: &PrinterApplicationCandidateSummary,
) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();
    let trailing_size = f32::from(spacing.space_l);
    let chevron: Element<'static, Message> = container(
        icon::from_name("go-next-symbolic")
            .size(crate::style::ICON_SIZE)
            .icon()
            .class(cosmic::theme::Svg::Custom(primary_svg())),
    )
    .width(Length::Fixed(trailing_size))
    .height(Length::Fixed(trailing_size))
    .center(Length::Fixed(trailing_size))
    .into();

    row_button(
        row::with_capacity(2)
            .align_y(Alignment::Center)
            .spacing(spacing.space_s)
            .push(row_label(candidate.printer_application_name.clone()))
            .push(chevron),
        APPLICATION_ROW_HEIGHT,
        Some(Message::SelectPrinterApplication(candidate.id.clone())),
    )
}

fn application_list(rows: Vec<Element<'static, Message>>) -> Element<'static, Message> {
    list_view(rows)
}

fn plain_row(label: String) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();

    container(row_label(label))
        .padding([0, spacing.space_m])
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .height(Length::Fixed(APPLICATION_ROW_HEIGHT))
        .into()
}

fn section_heading(label: String) -> Element<'static, Message> {
    text::body(label)
        .font(cosmic::font::bold())
        .width(Length::Fill)
        .into()
}

fn regular_heading(label: String) -> Element<'static, Message> {
    text::body(label).width(Length::Fill).into()
}

fn row_button(
    content: impl Into<Element<'static, Message>>,
    height: f32,
    on_press: Option<Message>,
) -> Element<'static, Message> {
    let spacing = cosmic::theme::spacing();

    container(content)
        .padding([spacing.space_xxs, spacing.space_m])
        .width(Length::Fill)
        .height(Length::Fixed(height))
        .class(cosmic::theme::Container::List)
        .apply(button::custom)
        .padding(0)
        .width(Length::Fill)
        .class(cosmic::theme::Button::Transparent)
        .on_press_maybe(on_press)
        .into()
}

fn row_label(label: String) -> Element<'static, Message> {
    text::body(label)
        .wrapping(Wrapping::None)
        .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)))
        .width(Length::Fill)
        .into()
}

fn list_view(rows: Vec<Element<'static, Message>>) -> Element<'static, Message> {
    column::with_children(rows)
        .spacing(0)
        .width(Length::Fill)
        .apply(container)
        .width(Length::Fill)
        .class(cosmic::theme::Container::List)
        .into()
}

fn with_dividers(rows: Vec<Element<'static, Message>>) -> Vec<Element<'static, Message>> {
    let mut divided = Vec::with_capacity(rows.len().saturating_mul(2).saturating_sub(1));
    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 {
            divided.push(widget::divider::horizontal::default().into());
        }
        divided.push(row);
    }
    divided
}

fn printer_display_name(printer: &PrinterEntry) -> String {
    [
        printer.name(),
        printer.model().unwrap_or_default(),
        printer.device_uri().unwrap_or_default(),
        printer.id(),
    ]
    .into_iter()
    .find(|value| !value.is_empty())
    .map(str::to_string)
    .unwrap_or_else(|| fl!("generic-printer"))
}

fn application_display_name(application: &ManualSetupPrinterApplication) -> String {
    non_empty(&application.display_name)
        .map(str::to_string)
        .unwrap_or_else(|| fl!("generic-printer-application"))
}

// Discovery results may include applications without a usable driver.
fn candidate_summary(printer: &DiscoveredPhysicalPrinter, every_answer_in: bool) -> String {
    let names = printer
        .candidates
        .iter()
        .filter(|candidate| is_selectable(candidate.state))
        .map(|candidate| candidate.printer_application_name.as_str())
        .collect::<Vec<_>>();

    if !names.is_empty() {
        return names.join(", ");
    }
    if !every_answer_in {
        return fl!("searching");
    }

    match printer.candidates.len() {
        0 => fl!("no-compatible-printer-applications"),
        count => fl!("seen-without-a-driver", count = count),
    }
}

fn is_selectable(state: PaCandidateState) -> bool {
    state == PaCandidateState::Ready
}

fn is_configured(printer: &DiscoveredPhysicalPrinter) -> bool {
    printer
        .candidates
        .iter()
        .any(|candidate| candidate.state == PaCandidateState::AlreadyConfigured)
}

fn selectable_candidate_ids(printer: &DiscoveredPhysicalPrinter) -> Vec<String> {
    printer
        .candidates
        .iter()
        .filter(|candidate| is_selectable(candidate.state))
        .map(|candidate| candidate.id.clone())
        .collect()
}

fn printer_matches_search(printer: &DiscoveredPhysicalPrinter, search: &str) -> bool {
    search.is_empty()
        || printer.display_name.to_lowercase().contains(search)
        || printer
            .make_and_model
            .as_deref()
            .is_some_and(|value| value.to_lowercase().contains(search))
        || printer.candidates.iter().any(|candidate| {
            candidate
                .printer_application_name
                .to_lowercase()
                .contains(search)
        })
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn primary_svg() -> Rc<dyn Fn(&cosmic::Theme) -> cosmic::widget::svg::Style> {
    Rc::new(|theme: &cosmic::Theme| cosmic::widget::svg::Style {
        color: Some(theme.cosmic().on_bg_color().into()),
    })
}

fn accent_svg() -> Rc<dyn Fn(&cosmic::Theme) -> cosmic::widget::svg::Style> {
    Rc::new(|theme: &cosmic::Theme| cosmic::widget::svg::Style {
        color: Some(theme.cosmic().accent_color().into()),
    })
}
