use cosmic::app::Task;
use cosmic::iced::border::Radius;
use cosmic::iced::core::text::{Ellipsize, EllipsizeHeightLimit, Wrapping};
use cosmic::iced::{Alignment, Color, Length};
use cosmic::widget::{
    self, column, container, row, settings, space::horizontal as horizontal_space, text,
};
use cosmic::{Apply, Element, surface};
use cosmic_settings_printers_core::{PrinterStatus, SupplyLevel, SupplyRgb, SupplyWarning};

use crate::style::{
    RADIUS_SUPPLY_BAR, SUPPLY_BAR_HEIGHT, SUPPLY_DOT_SIZE, SUPPLY_GRAPH_HEIGHT,
    SUPPLY_LABEL_HEIGHT, SUPPLY_MARK_HEIGHT, SUPPLY_MARK_WIDTH, SUPPLY_OUTLINE_TOLERANCE,
    SUPPLY_PERCENTAGE_WIDTH, SUPPLY_TRACK_HEIGHT,
};
use cosmic_settings_printers_core::PrinterEntry;

use crate::backend::Backend;

/// Messages handled by the printer details page.
#[derive(Clone, Debug)]
pub enum Message {
    /// Returns to the printer list.
    GoBack,
    /// Opens the location editor.
    EditLocation(String),
    /// Updates the location draft.
    EditLocationChanged(String),
    /// Saves the printer location.
    SubmitLocation(String, String),
    /// Closes the location editor.
    CancelDialog,
    /// Loads a printer into the page.
    LoadPrinter {
        /// Printer to display.
        printer: PrinterEntry,
        /// Whether this is the default printer.
        is_default: bool,
        /// Destinations available for moving jobs.
        available_printers: Vec<PrinterEntry>,
    },
    /// Opens the printer queue.
    OpenPrinterQueue(String),
    /// Removes the printer.
    RemovePrinter(String),
    /// Reports printer removal completion.
    PrinterDeleted(Result<(), String>),
    /// Selects a default paper size.
    SelectPaperSize(String, usize),
    /// Selects a default duplex mode.
    SelectPrintSides(String, usize),
    /// Reports an option-default update.
    PrinterOptionDefaultSet(Result<(), String>),
    /// Sets or clears the default printer.
    ToggleDefaultPrinter(String, bool),
    /// Reports a default-printer update.
    PrinterDefaultSet(Result<(), String>),
    /// Reports a location update.
    PrinterLocationSet(Result<(), String>),
    /// Reports loaded supply levels.
    SuppliesLoaded {
        /// Printer ID used to reject stale responses.
        printer_id: String,
        /// Loaded supplies or an error.
        result: Result<Vec<SupplyLevel>, String>,
    },
    /// Reports the active job count.
    ActiveJobsLoaded {
        /// Printer ID used to reject stale responses.
        printer_id: String,
        /// Job count or an error.
        result: Result<usize, String>,
    },
    /// Refreshes destination data.
    PrintersRefreshed(Vec<PrinterEntry>),
    /// Updates a popup surface.
    Surface(surface::Action),
}

/// Requests handled by the application shell.
#[derive(Clone, Debug)]
pub enum Request {
    /// Returns to the previous page.
    GoBack,
    /// Shows printer details.
    ShowDetails,
    /// Shows the print queue.
    ShowQueue,
    /// Updates a popup surface.
    Surface(surface::Action),
}

/// State for the printer details page.
#[derive(Default)]
pub struct State {
    backend: Backend,
    printer: Option<PrinterEntry>,
    available_printers: Vec<PrinterEntry>,
    dialog: Option<Dialog>,
    is_default: bool,
    supplies: Vec<SupplyLevel>,
    active_jobs: Option<usize>,
}

#[derive(Clone, Debug)]
enum Dialog {
    EditLocation {
        printer_id: String,
        location: String,
    },
}

impl State {
    /// Handles a printer details message.
    pub fn update<M>(&mut self, message: Message) -> Task<M>
    where
        M: 'static
            + Send
            + From<Message>
            + From<crate::list::Message>
            + From<crate::queue::Message>
            + From<Request>,
    {
        match message {
            Message::GoBack => self.go_back_task(),
            Message::LoadPrinter {
                printer,
                is_default,
                available_printers,
            } => {
                let printer_id = printer.id().to_string();
                self.load_printer(printer, is_default, available_printers);
                Task::batch([
                    load_supplies_task(self.backend.clone(), printer_id.clone()),
                    load_active_jobs_task(self.backend.clone(), printer_id),
                ])
            }
            Message::SuppliesLoaded { printer_id, result } => {
                self.apply_supplies(printer_id, result);
                Task::none()
            }
            Message::ActiveJobsLoaded { printer_id, result } => {
                self.apply_active_job_count(printer_id, result);
                Task::none()
            }
            Message::PrintersRefreshed(printers) => {
                if self.refresh_printers(printers) {
                    self.printer
                        .as_ref()
                        .map(|printer| {
                            load_active_jobs_task(self.backend.clone(), printer.id().to_string())
                        })
                        .unwrap_or_else(Task::none)
                } else {
                    Task::none()
                }
            }
            Message::CancelDialog => {
                self.dialog = None;
                Task::none()
            }
            Message::EditLocationChanged(location) => {
                self.update_location_draft(location);
                Task::none()
            }
            Message::SubmitLocation(printer_id, location) => {
                self.submit_location(printer_id, location)
            }
            Message::Surface(action) => cosmic::task::message(M::from(Request::Surface(action))),
            Message::ToggleDefaultPrinter(printer_id, true) => {
                self.is_default = true;
                Self::set_default_printer_task(self.backend.clone(), printer_id)
            }
            Message::ToggleDefaultPrinter(_, false) => {
                self.is_default = false;
                Self::clear_default_printer_task(self.backend.clone())
            }
            Message::RemovePrinter(printer_id) => {
                Self::delete_printer_task(self.backend.clone(), printer_id)
            }
            Message::PrinterDeleted(result) => self.finish_printer_deletion(result),
            Message::PrinterDefaultSet(result) => Self::finish_default_printer_update(result),
            Message::PrinterLocationSet(result) => Self::finish_location_update(result),
            Message::EditLocation(printer_id) => {
                self.open_location_dialog(printer_id);
                Task::none()
            }
            Message::SelectPaperSize(printer_id, index) => {
                self.select_paper_size(printer_id, index)
            }
            Message::SelectPrintSides(printer_id, index) => {
                self.select_print_sides(printer_id, index)
            }
            Message::PrinterOptionDefaultSet(result) => Self::finish_option_default_update(result),
            Message::OpenPrinterQueue(printer_id) => self.open_printer_queue(&printer_id),
        }
    }

    fn go_back_task<M>(&self) -> Task<M>
    where
        M: 'static + Send + From<Request>,
    {
        cosmic::task::message(M::from(Request::GoBack))
    }

    fn load_printer(
        &mut self,
        printer: PrinterEntry,
        is_default: bool,
        available_printers: Vec<PrinterEntry>,
    ) {
        self.supplies = printer.supplies();
        self.active_jobs = None;
        self.printer = Some(printer);
        self.is_default = is_default;
        self.available_printers = available_printers;
    }

    // Preserve dialog drafts while refreshing cached destination data.
    fn refresh_printers(&mut self, printers: Vec<PrinterEntry>) -> bool {
        let Some(shown) = self
            .printer
            .as_ref()
            .map(|printer| printer.id().to_string())
        else {
            self.available_printers = printers;
            return false;
        };

        let refreshed = printers.iter().find(|printer| printer.id() == shown).cloned();
        let changed = self.printer != refreshed;

        match refreshed {
            Some(refreshed) if changed => {
                self.supplies = refreshed.supplies();
                self.is_default = refreshed.is_default();
                self.printer = Some(refreshed);
            }
            None => {
                self.printer = None;
                self.supplies.clear();
                self.active_jobs = None;
                self.is_default = false;
            }
            Some(_) => {}
        }

        self.available_printers = printers;
        changed
    }

    fn waiting_documents(&self) -> String {
        match self.active_jobs {
            Some(0) => fl!("no-jobs-waiting"),
            Some(1) => fl!("one-document"),
            Some(count) => fl!("documents-count", count = count),
            None => String::new(),
        }
    }

    // Ignore responses for a printer that is no longer displayed.
    fn apply_supplies(&mut self, printer_id: String, result: Result<Vec<SupplyLevel>, String>) {
        if self.printer.as_ref().map(PrinterEntry::id) != Some(printer_id.as_str()) {
            return;
        }

        match result {
            Ok(supplies) => self.supplies = supplies,
            // Preserve cached levels when live loading fails.
            Err(why) => tracing::warn!(printer_id, why, "failed to load printer supplies"),
        }
    }

    // Ignore responses for a printer that is no longer displayed.
    fn apply_active_job_count(&mut self, printer_id: String, result: Result<usize, String>) {
        if self.printer.as_ref().map(PrinterEntry::id) != Some(printer_id.as_str()) {
            return;
        }

        match result {
            Ok(count) => self.active_jobs = Some(count),
            // A failed request does not establish that the queue is empty.
            Err(why) => tracing::warn!(printer_id, why, "failed to load active printer jobs"),
        }
    }

    fn update_location_draft(&mut self, location: String) {
        if let Some(Dialog::EditLocation {
            location: current, ..
        }) = &mut self.dialog
        {
            *current = location;
        }
    }

    fn submit_location<M>(&mut self, printer_id: String, location: String) -> Task<M>
    where
        M: 'static + Send + From<Message>,
    {
        self.dialog = None;

        if let Some(printer) = self
            .printer
            .as_mut()
            .filter(|printer| printer.id() == printer_id)
        {
            printer.set_location(location.clone());
        }
        let backend = self.backend.clone();

        cosmic::task::future(async move {
            M::from(Message::PrinterLocationSet(
                backend
                    .set_printer_location(&printer_id, &location)
                    .await
                    .map_err(|why| why.to_string()),
            ))
        })
    }

    fn set_default_printer_task<M>(backend: Backend, printer_id: String) -> Task<M>
    where
        M: 'static + Send + From<Message>,
    {
        cosmic::task::future(async move {
            M::from(Message::PrinterDefaultSet(
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
            M::from(Message::PrinterDefaultSet(
                backend
                    .clear_printer_default()
                    .await
                    .map_err(|why| why.to_string()),
            ))
        })
    }

    fn delete_printer_task<M>(backend: Backend, printer_id: String) -> Task<M>
    where
        M: 'static + Send + From<Message>,
    {
        cosmic::task::future(async move {
            M::from(Message::PrinterDeleted(
                backend
                    .delete_printer(&printer_id)
                    .await
                    .map_err(|why| why.to_string()),
            ))
        })
    }

    fn finish_printer_deletion<M>(&mut self, result: Result<(), String>) -> Task<M>
    where
        M: 'static + Send + From<crate::list::Message> + From<Request>,
    {
        match result {
            Ok(()) => {
                self.printer = None;
                cosmic::task::message(M::from(Request::GoBack))
            }
            Err(why) => {
                tracing::warn!(why, "failed to delete printer");
                Task::none()
            }
        }
    }

    fn finish_optimistic_change<M>(result: Result<(), String>, what: &str) -> Task<M>
    where
        M: 'static + Send + From<crate::list::Message>,
    {
        if let Err(why) = result {
            tracing::warn!(why, what, "a printer change was refused");
            return Self::refresh_printers_task();
        }

        Task::none()
    }

    fn finish_default_printer_update<M>(result: Result<(), String>) -> Task<M>
    where
        M: 'static + Send + From<crate::list::Message>,
    {
        Self::finish_optimistic_change(result, "default printer")
    }

    fn finish_location_update<M>(result: Result<(), String>) -> Task<M>
    where
        M: 'static + Send + From<crate::list::Message>,
    {
        Self::finish_optimistic_change(result, "location")
    }

    fn open_location_dialog(&mut self, printer_id: String) {
        let location = self
            .printer
            .as_ref()
            .filter(|printer| printer.id() == printer_id)
            .and_then(|printer| printer.location().map(str::to_owned))
            .unwrap_or_default();

        self.dialog = Some(Dialog::EditLocation {
            printer_id,
            location,
        });
    }

    fn select_paper_size<M>(&mut self, printer_id: String, index: usize) -> Task<M>
    where
        M: 'static + Send + From<Message>,
    {
        let value = self
            .printer
            .as_mut()
            .filter(|printer| printer.id() == printer_id)
            .and_then(|printer| {
                let value = printer.paper_sizes().get(index).cloned();
                if let Some(value) = &value {
                    printer.set_default_paper_size(value.clone());
                }
                value
            });

        let Some(value) = value else {
            return Task::none();
        };

        Self::set_option_default_task(self.backend.clone(), printer_id, "media".into(), value)
    }

    fn select_print_sides<M>(&mut self, printer_id: String, index: usize) -> Task<M>
    where
        M: 'static + Send + From<Message>,
    {
        let value = self
            .printer
            .as_mut()
            .filter(|printer| printer.id() == printer_id)
            .and_then(|printer| {
                let value = printer.print_sides().get(index).cloned();
                if let Some(value) = &value {
                    printer.set_default_print_sides(value.clone());
                }
                value
            });

        let Some(value) = value else {
            return Task::none();
        };

        Self::set_option_default_task(self.backend.clone(), printer_id, "sides".into(), value)
    }

    fn set_option_default_task<M>(
        backend: Backend,
        printer_id: String,
        option: String,
        value: String,
    ) -> Task<M>
    where
        M: 'static + Send + From<Message>,
    {
        cosmic::task::future(async move {
            M::from(Message::PrinterOptionDefaultSet(
                backend
                    .set_printer_option_default(&printer_id, &option, &[value])
                    .await
                    .map_err(|why| why.to_string()),
            ))
        })
    }

    fn finish_option_default_update<M>(result: Result<(), String>) -> Task<M>
    where
        M: 'static + Send + From<crate::list::Message>,
    {
        Self::finish_optimistic_change(result, "option default")
    }

    fn open_printer_queue<M>(&self, printer_id: &str) -> Task<M>
    where
        M: 'static + Send + From<crate::queue::Message> + From<Request>,
    {
        let Some(printer) = self
            .printer
            .as_ref()
            .filter(|printer| printer.id() == printer_id)
        else {
            return Task::none();
        };

        Task::batch([
            cosmic::task::message(M::from(crate::queue::Message::LoadPrinter {
                printer: Box::new(printer.clone()),
                available_printers: self.available_printers.clone(),
            })),
            cosmic::task::message(M::from(Request::ShowQueue)),
        ])
    }

    fn refresh_printers_task<M>() -> Task<M>
    where
        M: 'static + Send + From<crate::list::Message>,
    {
        cosmic::task::message(M::from(crate::list::Message::Refresh))
    }
}

/// Returns the printer details header.
pub fn header_view(state: &State) -> Option<Element<'_, Message>> {
    state.printer.as_ref().map(details_header)
}

/// Returns the active details dialog.
pub fn dialog_view(state: &State) -> Option<Element<'_, Message>> {
    state.dialog.as_ref().map(|dialog| match dialog {
        Dialog::EditLocation {
            printer_id,
            location,
        } => {
            let input = widget::text_input("", location)
                .on_input(Message::EditLocationChanged)
                .on_submit({
                    let printer_id = printer_id.clone();
                    move |location| Message::SubmitLocation(printer_id.clone(), location)
                });

            let primary_action = widget::button::suggested(fl!("save")).on_press(
                Message::SubmitLocation(printer_id.clone(), location.clone()),
            );
            let secondary_action =
                widget::button::standard(fl!("cancel")).on_press(Message::CancelDialog);

            widget::dialog()
                .title(fl!("location"))
                .control(input)
                .primary_action(primary_action)
                .secondary_action(secondary_action)
                .apply(Element::from)
        }
    })
}

/// Returns the empty details view.
pub fn nothing_selected_view() -> Element<'static, Message> {
    text::body(fl!("no-printer-selected")).apply(Element::from)
}

/// Returns the default-printer and queue section.
pub fn default_and_queue_view<'a>(state: &'a State, title: &'a str) -> Element<'a, Message> {
    let Some(printer) = state.printer.as_ref() else {
        return Element::from(horizontal_space());
    };
    let id = printer.id().to_string();

    settings::section()
        .title(title)
        .add(
            settings::item::builder(fl!("set-as-default-printer"))
                .toggler(state.is_default, move |value| {
                    Message::ToggleDefaultPrinter(id.clone(), value)
                }),
        )
        .add(queue_item(
            fl!("printer-queue"),
            state.waiting_documents(),
            Message::OpenPrinterQueue(printer.id().to_string()),
        ))
        .apply(Element::from)
}

/// Returns the printer information section.
pub fn printer_information_view<'a>(state: &'a State, title: &'a str) -> Element<'a, Message> {
    let Some(printer) = state.printer.as_ref() else {
        return Element::from(horizontal_space());
    };

    settings::section()
        .title(title)
        .add(settings::item(fl!("location"), location_value(printer)))
        .add(settings::item(
            fl!("model"),
            value_text(printer.model().unwrap_or_default().to_string()),
        ))
        .add(settings::item(
            fl!("device-name"),
            value_text(printer.name().to_string()),
        ))
        .add(settings::item(
            fl!("driver-version"),
            value_text(printer.driver_version().unwrap_or_default().to_string()),
        ))
        .apply(Element::from)
}

/// Returns the printer preferences section.
pub fn printer_preferences_view<'a, M: 'static + Clone>(
    state: &'a State,
    title: &'a str,
    to_host: fn(Message) -> M,
) -> Element<'a, Message> {
    let Some(printer) = state.printer.as_ref() else {
        return Element::from(horizontal_space());
    };

    let sizes = printer.paper_sizes();
    let sides = printer.print_sides();
    let size_labels = sizes.iter().map(|value| media_label(value)).collect();
    let side_labels = sides.iter().map(|value| sides_label(value)).collect();

    settings::section()
        .title(title)
        .add(settings::item(
            fl!("paper-size"),
            option_dropdown(
                size_labels,
                selected_option_idx(&sizes, printer.default_paper_size(), 0),
                {
                    let id = printer.id().to_string();
                    move |index| Message::SelectPaperSize(id.clone(), index)
                },
                to_host,
            ),
        ))
        .add(settings::item(
            fl!("print-sides"),
            option_dropdown(
                side_labels,
                selected_option_idx(&sides, printer.default_print_sides(), 0),
                {
                    let id = printer.id().to_string();
                    move |index| Message::SelectPrintSides(id.clone(), index)
                },
                to_host,
            ),
        ))
        .apply(Element::from)
}

/// Returns the printer supplies section.
pub fn supplies_view<'a>(state: &'a State, title: &'a str) -> Element<'a, Message> {
    settings::section()
        .title(title)
        .add(supply_grid(&state.supplies))
        .apply(Element::from)
}

/// Returns the remove-printer action.
pub fn remove_printer_view(state: &State) -> Element<'_, Message> {
    let Some(printer) = state.printer.as_ref() else {
        return Element::from(horizontal_space());
    };

    widget::button::destructive(fl!("remove-printer"))
        .on_press(Message::RemovePrinter(printer.id().to_string()))
        .apply(container)
        .width(Length::Fill)
        .align_x(Alignment::End)
        .apply(Element::from)
}

impl State {
    /// Sets the printer backend.
    pub fn set_backend(&mut self, backend: Backend) {
        self.backend = backend;
    }

    /// Returns whether a printer is selected.
    #[must_use]
    pub fn has_printer(&self) -> bool {
        self.printer.is_some()
    }

    /// Returns whether the printer reported supplies.
    #[must_use]
    pub fn has_supplies(&self) -> bool {
        !self.supplies.is_empty()
    }

    /// Returns whether the printer can be removed.
    #[must_use]
    pub fn can_remove_printer(&self) -> bool {
        self.printer
            .as_ref()
            .is_some_and(PrinterEntry::can_administer)
    }
}

fn option_dropdown<M: 'static + Clone>(
    labels: Vec<String>,
    selected: usize,
    select: impl Fn(usize) -> Message + Send + Sync + 'static,
    to_host: fn(Message) -> M,
) -> Element<'static, Message> {
    widget::dropdown::popup_dropdown(
        labels,
        Some(selected),
        select,
        cosmic::iced::window::Id::RESERVED,
        Message::Surface,
        to_host,
    )
    .into()
}

fn details_header(printer: &PrinterEntry) -> Element<'static, Message> {
    let spacing = cosmic::theme::active().cosmic().spacing;

    column::with_capacity(3)
        .width(Length::Fill)
        .spacing(spacing.space_xxs)
        .push(back_button())
        .push(
            text::title3(printer.name().to_string())
                .wrapping(Wrapping::None)
                .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1))),
        )
        .push(status_line(&printer.status()))
        .apply(Element::from)
}

fn back_button() -> Element<'static, Message> {
    widget::button::custom(
        row::with_capacity(2)
            .align_y(Alignment::Center)
            .spacing(cosmic::theme::active().cosmic().space_xxxs())
            .push(widget::icon::from_name("go-previous-symbolic").size(crate::style::ICON_SIZE))
            .push(text::body(fl!("printers"))),
    )
    .class(cosmic::theme::Button::Link)
    .on_press(Message::GoBack)
    .into()
}

fn status_line(status: &PrinterStatus) -> Element<'static, Message> {
    let label = match status {
        PrinterStatus::Ready => fl!("printer-ready"),
        PrinterStatus::Offline => fl!("printer-offline"),
        PrinterStatus::LowToner => fl!("printer-low-toner"),
    };

    row::with_capacity(2)
        .align_y(Alignment::Center)
        .spacing(cosmic::theme::active().cosmic().space_xxs())
        .push(crate::widgets::dot(status_color(status), 8.0))
        .push(text::body(label))
        .apply(Element::from)
}

fn status_color(status: &PrinterStatus) -> Color {
    match status {
        PrinterStatus::Ready => crate::style::status_ready(),
        PrinterStatus::Offline | PrinterStatus::LowToner => crate::style::status_stopped(),
    }
}

fn selected_option_idx(values: &[String], default: Option<&str>, fallback: usize) -> usize {
    default
        .and_then(|default| values.iter().position(|value| value == default))
        .unwrap_or(fallback)
        .min(values.len().saturating_sub(1))
}

fn media_label(value: &str) -> String {
    let Some((name, size)) = media_name_and_size(value) else {
        return value.to_string();
    };

    format!("{name} ({size})")
}

fn media_name_and_size(value: &str) -> Option<(String, String)> {
    let (size_raw, unit) = value
        .strip_suffix("mm")
        .map(|size| (size, "mm"))
        .or_else(|| value.strip_suffix("in").map(|size| (size, "in")))?;
    let size_start = size_raw.rfind('_')? + 1;
    let dimensions = &size_raw[size_start..];
    let name_end = size_start.saturating_sub(1);
    let name_raw = value
        .get(..name_end)?
        .rsplit_once('_')
        .map(|(_, name)| name)
        .unwrap_or(value.get(..name_end)?);

    Some((
        pretty_media_name(name_raw),
        format!(
            "{} {}",
            dimensions.replace('x', " x "),
            if unit == "in" { "inches" } else { unit }
        ),
    ))
}

fn pretty_media_name(name: &str) -> String {
    match name {
        "a0" | "a1" | "a2" | "a3" | "a4" | "a5" | "a6" => name.to_uppercase(),
        "b0" | "b1" | "b2" | "b3" | "b4" | "b5" => name.to_uppercase(),
        "c0" | "c1" | "c2" | "c3" | "c4" | "c5" => name.to_uppercase(),
        "dl" => "DL".into(),
        other => other
            .split(['-', '_'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) if part.chars().any(char::is_alphabetic) => {
                        format!("{}{}", first.to_uppercase(), chars.as_str())
                    }
                    Some(_) => part.to_string(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn sides_label(value: &str) -> String {
    match value {
        "one-sided" => fl!("print-one-side"),
        "two-sided-long-edge" => fl!("print-both-sides"),
        "two-sided-short-edge" => fl!("print-both-sides"),
        _ => value.to_string(),
    }
}

fn load_supplies_task<M>(backend: Backend, printer_id: String) -> Task<M>
where
    M: 'static + Send + From<Message>,
{
    cosmic::task::future(async move {
        let result = backend
            .printer_supplies(&printer_id)
            .await
            .map_err(|why| why.to_string());
        M::from(Message::SuppliesLoaded { printer_id, result })
    })
}

fn load_active_jobs_task<M>(backend: Backend, printer_id: String) -> Task<M>
where
    M: 'static + Send + From<Message>,
{
    cosmic::task::future(async move {
        let result = backend
            .jobs(
                &printer_id,
                cosmic_settings_printers_core::JobFilter::Active,
            )
            .await
            .map(|jobs| jobs.len())
            .map_err(|why| why.to_string());
        M::from(Message::ActiveJobsLoaded { printer_id, result })
    })
}

const SUPPLY_COLUMNS: usize = 2;

fn supply_grid(supplies: &[SupplyLevel]) -> Element<'static, Message> {
    let spacing = cosmic::theme::active().cosmic().spacing;
    let mut grid = column::with_capacity(supply_rows(supplies.len()))
        .width(Length::Fill)
        .spacing(spacing.space_xs);

    for chunk in supplies.chunks(SUPPLY_COLUMNS) {
        let mut cells = row::with_capacity(SUPPLY_COLUMNS)
            .width(Length::Fill)
            .height(Length::Fixed(SUPPLY_GRAPH_HEIGHT))
            .spacing(spacing.space_s);

        for supply in chunk {
            cells = cells.push(supply_graph(supply));
        }
        // Preserve the empty grid cell so a final item does not stretch.
        for _ in chunk.len()..SUPPLY_COLUMNS {
            cells = cells.push(horizontal_space());
        }

        grid = grid.push(cells);
    }

    grid.into()
}

fn supply_rows(supplies: usize) -> usize {
    supplies.div_ceil(SUPPLY_COLUMNS)
}

// Location changes require a managed queue and scheduler administration access.
fn location_value(printer: &PrinterEntry) -> Element<'static, Message> {
    let location = printer.location().unwrap_or_default().to_string();

    if !printer.can_administer() {
        return value_text(location);
    }

    row::with_capacity(2)
        .align_y(Alignment::Center)
        .spacing(cosmic::theme::active().cosmic().space_xxs())
        .push(value_text(location))
        .push(
            widget::button::icon(widget::icon::from_name(crate::icons::edit()))
                .on_press(Message::EditLocation(printer.id().to_string())),
        )
        .apply(Element::from)
}

fn queue_item(label: String, value: String, message: Message) -> Element<'static, Message> {
    settings::item(
        label,
        row::with_capacity(2)
            .align_y(Alignment::Center)
            .spacing(cosmic::theme::active().cosmic().space_xxs())
            .push(value_text(value))
            .push(
                widget::button::icon(widget::icon::from_name("go-next-symbolic")).on_press(message),
            ),
    )
    .apply(Element::from)
}

fn value_text(value: String) -> Element<'static, Message> {
    text::body(value)
        .class(cosmic::theme::Text::Default)
        .align_x(Alignment::End)
        .wrapping(Wrapping::None)
        .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)))
        .into()
}

fn supply_graph(supply: &SupplyLevel) -> Element<'static, Message> {
    let colors = bar_colors(supply);

    column::with_capacity(2)
        .width(Length::Fill)
        .height(Length::Fixed(SUPPLY_GRAPH_HEIGHT))
        .push(
            row::with_capacity(2)
                .height(Length::Fixed(SUPPLY_LABEL_HEIGHT))
                .align_y(Alignment::Center)
                .spacing(8)
                .push(
                    text::body(supply_name(supply))
                        .width(Length::Fill)
                        .wrapping(Wrapping::None)
                        .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1))),
                )
                .push_maybe((colors.len() > 1).then(|| color_dots(&colors))),
        )
        .push(
            row::with_capacity(2)
                .height(Length::Fixed(SUPPLY_BAR_HEIGHT))
                .align_y(Alignment::Center)
                .spacing(0)
                .push(progress_track(supply, &colors))
                .push(supply_percentage(supply.level_percent)),
        )
        .into()
}

fn supply_name(supply: &SupplyLevel) -> String {
    if let [color] = supply.colors.as_slice()
        && let Some(name) = known_supply_color_name(*color)
    {
        return name;
    }

    if supply.name.is_empty() {
        fl!("supply-unnamed")
    } else {
        supply.name.clone()
    }
}

fn known_supply_color_name(color: SupplyRgb) -> Option<String> {
    match (color.red, color.green, color.blue) {
        (0x00, 0x00, 0x00) => Some(fl!("supply-color-black")),
        (0x00, 0xFF, 0xFF) => Some(fl!("supply-color-cyan")),
        (0xFF, 0x00, 0xFF) => Some(fl!("supply-color-magenta")),
        (0xFF, 0xFF, 0x00) => Some(fl!("supply-color-yellow")),
        (0xE0, 0xFF, 0xFF) => Some(fl!("supply-color-light-cyan")),
        (0xFF, 0xE0, 0xFF) => Some(fl!("supply-color-light-magenta")),
        (0x80, 0x80, 0x80) => Some(fl!("supply-color-gray")),
        (0xFF, 0x00, 0x00) => Some(fl!("supply-color-red")),
        (0x00, 0xFF, 0x00) => Some(fl!("supply-color-green")),
        (0x00, 0x00, 0xFF) => Some(fl!("supply-color-blue")),
        (0xFF, 0xA5, 0x00) => Some(fl!("supply-color-orange")),
        (0xEE, 0x82, 0xEE) => Some(fl!("supply-color-violet")),
        (0xFF, 0xFF, 0xFF) => Some(fl!("supply-color-white")),
        _ => None,
    }
}

fn supply_percentage(level: Option<u8>) -> Element<'static, Message> {
    container(
        text::body(percentage_label(level))
            .wrapping(Wrapping::None)
            .align_x(Alignment::Start),
    )
    .width(Length::Fixed(SUPPLY_PERCENTAGE_WIDTH))
    .height(Length::Fixed(SUPPLY_BAR_HEIGHT))
    .padding([0, 0, 0, cosmic::theme::active().cosmic().space_xxs()])
    .align_y(Alignment::Center)
    .into()
}

fn percentage_label(level: Option<u8>) -> String {
    match level {
        Some(level) if level >= 100 => "100%".to_string(),
        Some(level) => format!("{:.1}%", f32::from(level)),
        None => fl!("supply-level-unknown"),
    }
}

// Overlay the warning mark without splitting the rounded progress track.
fn progress_track(supply: &SupplyLevel, colors: &[Color]) -> Element<'static, Message> {
    let track = container(supply_fill(supply.level_percent, colors))
        .width(Length::Fill)
        .height(Length::Fixed(SUPPLY_TRACK_HEIGHT))
        .class(crate::widgets::fill_container(
            crate::style::supply_track(),
            RADIUS_SUPPLY_BAR,
        ));

    let Some(warning) = supply.warning else {
        return track.into();
    };

    cosmic::iced::widget::stack![
        container(track)
            .width(Length::Fill)
            .height(Length::Fixed(SUPPLY_BAR_HEIGHT))
            .align_y(Alignment::Center),
        warning_mark(warning, supply.level_percent),
    ]
    .width(Length::Fill)
    .height(Length::Fixed(SUPPLY_BAR_HEIGHT))
    .into()
}

fn supply_fill(level: Option<u8>, colors: &[Color]) -> Element<'static, Message> {
    let mut bar = row::with_capacity(2).height(Length::Fixed(SUPPLY_TRACK_HEIGHT));
    let filled = level.unwrap_or(0).min(100);
    let empty = 100_u8.saturating_sub(filled);

    if filled > 0 {
        bar = bar.push(
            container(band(supply_fill_color(colors)))
                .width(Length::FillPortion(u16::from(filled)))
                .height(Length::Fixed(SUPPLY_TRACK_HEIGHT)),
        );
    }

    // A zero `FillPortion` expands to full width, so omit it.
    if empty > 0 {
        bar = bar.push(horizontal_space().width(Length::FillPortion(u16::from(empty))));
    }

    bar.into()
}

fn supply_fill_color(colors: &[Color]) -> Color {
    match colors {
        [] => crate::style::supply_neutral(),
        [only] => *only,
        _ => crate::style::status_printing(),
    }
}

fn band(color: Color) -> container::Container<'static, Message, cosmic::Theme> {
    let radius = Radius::from(RADIUS_SUPPLY_BAR);
    let style = if needs_outline(color) {
        crate::widgets::bordered_fill_container(color, crate::style::hairline(), radius)
    } else {
        crate::widgets::fill_container(color, radius)
    };

    container(horizontal_space())
        .width(Length::Fill)
        .height(Length::Fixed(SUPPLY_TRACK_HEIGHT))
        .class(style)
}

fn warning_mark(warning: SupplyWarning, level: Option<u8>) -> Element<'static, Message> {
    let reached = level.is_some_and(|level| warning.is_reached_by(level));
    let before = warning.level_percent.min(100);
    let after = 100_u8.saturating_sub(before);
    let mut marks = row::with_capacity(3)
        .width(Length::Fill)
        .height(Length::Fixed(SUPPLY_BAR_HEIGHT))
        .align_y(Alignment::Center);

    if before > 0 {
        marks = marks.push(horizontal_space().width(Length::FillPortion(u16::from(before))));
    }

    marks = marks.push(
        container(horizontal_space())
            .width(Length::Fixed(SUPPLY_MARK_WIDTH))
            .height(Length::Fixed(SUPPLY_MARK_HEIGHT))
            .class(crate::widgets::fill_container(
                if reached {
                    crate::style::status_stopped()
                } else {
                    cosmic::theme::active().cosmic().on_bg_color().into()
                },
                1.0,
            )),
    );

    if after > 0 {
        marks = marks.push(horizontal_space().width(Length::FillPortion(u16::from(after))));
    }

    marks.into()
}

fn bar_colors(supply: &SupplyLevel) -> Vec<Color> {
    supply
        .colors
        .iter()
        .map(|color| crate::style::visible_on_card(supply_color(*color)))
        .collect()
}

fn supply_color(color: SupplyRgb) -> Color {
    Color::from_rgba8(color.red, color.green, color.blue, 1.0)
}

fn needs_outline(color: Color) -> bool {
    let track = crate::style::supply_track();
    let peak = color.r.max(color.g).max(color.b);
    let track_peak = track.r.max(track.g).max(track.b);

    (peak - track_peak).abs() < SUPPLY_OUTLINE_TOLERANCE
}

fn color_dots(colors: &[Color]) -> Element<'static, Message> {
    let mut dots = row::with_capacity(colors.len())
        .height(Length::Fixed(SUPPLY_DOT_SIZE))
        .spacing(cosmic::theme::active().cosmic().space_xxxs());

    for color in colors {
        dots = dots.push(color_dot(*color));
    }

    dots.into()
}

fn color_dot(color: Color) -> Element<'static, Message> {
    container(horizontal_space())
        .width(Length::Fixed(SUPPLY_DOT_SIZE))
        .height(Length::Fixed(SUPPLY_DOT_SIZE))
        .class(crate::widgets::bordered_fill_container(
            color,
            crate::style::hairline(),
            SUPPLY_DOT_SIZE / 2.0,
        ))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn supply(name: &str, colors: Vec<SupplyRgb>) -> SupplyLevel {
        SupplyLevel {
            name: name.to_string(),
            level_percent: Some(50),
            colors,
            warning: None,
        }
    }

    fn channels(color: Color) -> [u8; 3] {
        [
            (color.r * 255.0).round() as u8,
            (color.g * 255.0).round() as u8,
            (color.b * 255.0).round() as u8,
        ]
    }

    #[test]
    fn a_supply_too_dark_to_see_is_lifted() {
        assert_eq!(
            channels(crate::style::visible_on_card(Color::BLACK)),
            [0x9A, 0x9A, 0x9A]
        );

        for bright in [
            Color::from_rgba8(0x00, 0xFF, 0xFF, 1.0),
            Color::from_rgba8(0xFF, 0x00, 0xFF, 1.0),
            Color::from_rgba8(0xFF, 0xFF, 0x00, 1.0),
        ] {
            assert_eq!(
                channels(crate::style::visible_on_card(bright)),
                channels(bright)
            );
        }
    }

    #[test]
    fn lifting_a_colour_keeps_its_hue() {
        let lifted = crate::style::visible_on_card(Color::from_rgba8(0x00, 0x00, 0x80, 1.0));

        assert_eq!(channels(lifted), [0x00, 0x00, 0x9A]);
    }

    #[test]
    fn a_supply_the_colour_of_the_track_is_outlined() {
        assert!(needs_outline(crate::style::supply_track()));
        assert!(!needs_outline(crate::style::supply_neutral()));
        assert!(!needs_outline(Color::from_rgba8(0x00, 0xFF, 0xFF, 1.0)));
    }

    #[test]
    fn supplies_fill_rows_two_at_a_time() {
        assert_eq!(
            (1..=5).map(supply_rows).collect::<Vec<_>>(),
            [1, 1, 2, 2, 3]
        );
    }

    #[test]
    fn rows_keep_the_order_the_printer_reported() {
        let supplies = [0, 1, 2, 3, 4];
        let rows = supplies
            .chunks(SUPPLY_COLUMNS)
            .map(<[i32]>::to_vec)
            .collect::<Vec<_>>();

        assert_eq!(rows, [vec![0, 1], vec![2, 3], vec![4]]);
        assert_eq!(rows.concat(), supplies);
    }

    #[test]
    fn a_supply_of_several_colours_is_drawn_in_the_accent() {
        let cyan = Color::from_rgba8(0x00, 0xFF, 0xFF, 1.0);
        let magenta = Color::from_rgba8(0xFF, 0x00, 0xFF, 1.0);
        let yellow = Color::from_rgba8(0xFF, 0xFF, 0x00, 1.0);

        assert_eq!(
            channels(supply_fill_color(&[cyan, magenta, yellow])),
            channels(crate::style::status_printing())
        );
        assert_eq!(channels(supply_fill_color(&[cyan])), channels(cyan));
        assert_eq!(
            channels(supply_fill_color(&[])),
            channels(crate::style::supply_neutral())
        );
    }

    #[test]
    fn a_full_supply_is_written_without_a_decimal() {
        assert_eq!(percentage_label(Some(100)), "100%");
        assert_eq!(percentage_label(Some(92)), "92.0%");
        assert_eq!(percentage_label(Some(0)), "0.0%");
    }

    #[test]
    fn known_single_colors_replace_the_reported_supply_name() {
        let known = [
            ((0x00, 0x00, 0x00), fl!("supply-color-black")),
            ((0x00, 0xFF, 0xFF), fl!("supply-color-cyan")),
            ((0xFF, 0x00, 0xFF), fl!("supply-color-magenta")),
            ((0xFF, 0xFF, 0x00), fl!("supply-color-yellow")),
            ((0xE0, 0xFF, 0xFF), fl!("supply-color-light-cyan")),
            ((0xFF, 0xE0, 0xFF), fl!("supply-color-light-magenta")),
            ((0x80, 0x80, 0x80), fl!("supply-color-gray")),
            ((0xFF, 0x00, 0x00), fl!("supply-color-red")),
            ((0x00, 0xFF, 0x00), fl!("supply-color-green")),
            ((0x00, 0x00, 0xFF), fl!("supply-color-blue")),
            ((0xFF, 0xA5, 0x00), fl!("supply-color-orange")),
            ((0xEE, 0x82, 0xEE), fl!("supply-color-violet")),
            ((0xFF, 0xFF, 0xFF), fl!("supply-color-white")),
        ];

        for ((red, green, blue), expected) in known {
            let item = supply(
                "Cartridge ink-cartridge S/N:123456",
                vec![SupplyRgb { red, green, blue }],
            );
            assert_eq!(supply_name(&item), expected);
        }
    }

    #[test]
    fn unknown_and_multiple_colors_keep_the_reported_supply_name() {
        let unknown = supply(
            "Vendor spot ink S/N:123456",
            vec![SupplyRgb {
                red: 0x12,
                green: 0x34,
                blue: 0x56,
            }],
        );
        assert_eq!(supply_name(&unknown), "Vendor spot ink S/N:123456");

        let multiple = supply(
            "Combined color cartridge S/N:654321",
            vec![
                SupplyRgb {
                    red: 0x00,
                    green: 0xFF,
                    blue: 0xFF,
                },
                SupplyRgb {
                    red: 0xFF,
                    green: 0x00,
                    blue: 0xFF,
                },
            ],
        );
        assert_eq!(
            supply_name(&multiple),
            "Combined color cartridge S/N:654321"
        );
    }
}
