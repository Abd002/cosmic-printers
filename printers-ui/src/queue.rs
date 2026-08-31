//! Print queue UI.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::backend::Backend;
use cosmic::app::Task;
use cosmic::iced::core::text::{Ellipsize, EllipsizeHeightLimit, Wrapping};
use cosmic::iced::keyboard::Modifiers;
use cosmic::iced::{Alignment, Color, Length, Point};
use cosmic::widget::{self, column, container, menu, row, scrollable, text};
use cosmic::{Apply, Element};
use cosmic_settings_printers_core::{JobFilter, JobInfo, JobState, PrinterEntry};

use crate::{backend, style, widgets};

const QUEUE_CONTENT_PADDING: [u16; 4] = [0, 32, 32, 32];
const QUEUE_ROW_PADDING: [u16; 2] = [12, 24];
const QUEUE_ROW_SPACING: u16 = 16;
const QUEUE_CONTROLS_WIDTH: f32 = 64.0;
const QUEUE_MENU_WIDTH: f32 = 360.0;
const QUEUE_MENU_ROW_HEIGHT: f32 = 40.0;
const QUEUE_DESTINATION_MENU_MAX_HEIGHT: f32 = 320.0;
// The drawer's scrollable parent provides unbounded height, so `Length::Fill` would collapse.
const QUEUE_SURFACE_HEIGHT: f32 = 600.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JobAction {
    Pause,
    Resume,
    Cancel,
}

impl JobAction {
    fn is_available_for(self, state: &JobState) -> bool {
        match self {
            Self::Pause => matches!(state, JobState::Pending | JobState::Processing),
            Self::Resume => matches!(state, JobState::Held | JobState::Stopped),
            Self::Cancel => !matches!(
                state,
                JobState::Completed | JobState::Canceled | JobState::Aborted | JobState::Failed
            ),
        }
    }
}

/// CUPS job identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JobId(i32);

impl JobId {
    const fn from_raw(raw: i32) -> Self {
        Self(raw)
    }

    const fn into_raw(self) -> i32 {
        self.0
    }
}

/// Operation on a non-empty set of jobs.
///
/// Captures the selected jobs so later selection changes cannot affect the operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobOperation {
    action: JobAction,
    job_ids: Vec<JobId>,
}

impl JobOperation {
    fn new(action: JobAction, job_ids: impl IntoIterator<Item = JobId>) -> Option<Self> {
        let job_ids = job_ids.into_iter().collect::<Vec<_>>();
        (!job_ids.is_empty()).then_some(Self { action, job_ids })
    }

    fn single(action: JobAction, job_id: JobId) -> Self {
        Self {
            action,
            job_ids: vec![job_id],
        }
    }

    fn is_available_for(&self, jobs: &[JobInfo]) -> bool {
        self.job_ids.iter().all(|job_id| {
            jobs.iter().any(|job| {
                JobId::from_raw(job.id) == *job_id && self.action.is_available_for(&job.state)
            })
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum QueueMenu {
    SelectedJobs,
    Whole,
    MoveToPrinter { job_ids: Vec<JobId> },
}

/// Messages handled by the print queue.
#[derive(Clone, Debug)]
pub enum Message {
    /// Loads a printer queue.
    LoadPrinter {
        /// Printer whose jobs are displayed.
        printer: Box<PrinterEntry>,
        /// Destinations available for moving jobs.
        available_printers: Vec<PrinterEntry>,
    },
    /// Reports loaded jobs.
    JobsLoaded {
        /// Printer ID used to reject stale responses.
        printer_id: String,
        /// Loaded jobs or an error.
        result: Result<Vec<JobInfo>, String>,
    },
    /// Selects a job using the active modifiers.
    SelectJob(JobId),
    /// Clears the job selection.
    ClearSelection,
    /// Updates the cursor position.
    CursorMoved(Point),
    /// Updates selection modifiers supplied by the host page.
    ModifiersChanged(Modifiers),
    /// Opens a job context menu.
    OpenJobMenu(JobId),
    /// Opens the queue context menu.
    OpenWholeQueueMenu,
    /// Opens the move-destination menu.
    OpenMoveToPrinter(Vec<JobId>),
    /// Closes the active menu.
    CloseMenu,
    /// Moves jobs to another printer.
    MoveJobs {
        /// Destination printer ID.
        destination_id: String,
        /// Jobs to move.
        job_ids: Vec<JobId>,
    },
    /// Runs a job operation.
    RunJobAction(JobOperation),
    /// Reports a completed job operation.
    JobActionFinished {
        /// Printer queue to refresh.
        printer_id: String,
        /// Operation result.
        result: Result<(), String>,
    },
    /// Prints a test page.
    PrintTestPage,
    /// Reports test-page submission.
    TestPageFinished {
        /// Printer queue to refresh.
        printer_id: String,
        /// Submitted job ID or an error.
        result: Result<i32, String>,
    },
    /// Refreshes jobs.
    Refresh,
    /// Toggles completed jobs.
    ToggleCompleted,
    /// Opens the printer web interface.
    OpenPrinterWebPage(String),
    /// Reports whether the web interface opened.
    PrinterWebPageOpened(Result<(), String>),
}

/// State for the print queue.
#[derive(Default)]
pub struct State {
    backend: Backend,
    printer: Option<PrinterEntry>,
    available_printers: Vec<PrinterEntry>,
    jobs: Vec<JobInfo>,
    loading: bool,
    error: Option<String>,
    action_error: Option<String>,
    selected_jobs: HashSet<JobId>,
    selection_anchor: Option<JobId>,
    show_completed: bool,
    modifiers: Modifiers,
    operation_in_flight: bool,
    menu: Option<QueueMenu>,
    menu_position: Point,
    cursor_position: Point,
}

impl State {
    /// Handles a print queue message.
    pub fn update<M>(&mut self, message: Message) -> Task<M>
    where
        M: 'static + Send + From<Message> + From<crate::list::Message>,
    {
        match message {
            Message::LoadPrinter {
                printer,
                available_printers,
            } => self.load_printer(*printer, available_printers),
            Message::JobsLoaded { printer_id, result } => {
                self.apply_jobs_loaded(printer_id, result)
            }
            Message::SelectJob(job_id) => {
                self.select_job(job_id);
                Task::none()
            }
            Message::ClearSelection => {
                self.clear_selection();
                Task::none()
            }
            Message::CursorMoved(position) => {
                self.cursor_position = position;
                Task::none()
            }
            Message::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers;
                Task::none()
            }
            Message::OpenJobMenu(job_id) => {
                self.open_job_menu(job_id);
                Task::none()
            }
            Message::OpenWholeQueueMenu => {
                self.open_whole_queue_menu();
                Task::none()
            }
            Message::OpenMoveToPrinter(job_ids) => {
                self.open_move_to_printer(job_ids);
                Task::none()
            }
            Message::CloseMenu => {
                self.menu = None;
                Task::none()
            }
            Message::MoveJobs {
                destination_id,
                job_ids,
            } => self.start_move_jobs(destination_id, job_ids),
            Message::RunJobAction(operation) => self.start_job_action(operation),
            Message::JobActionFinished { printer_id, result } => {
                self.finish_job_action(printer_id, result)
            }
            Message::PrintTestPage => self.start_test_page(),
            Message::TestPageFinished { printer_id, result } => {
                self.finish_test_page(printer_id, result)
            }
            Message::Refresh => self.load_jobs_task(),
            Message::ToggleCompleted => {
                self.show_completed = !self.show_completed;
                self.clear_selection();
                self.load_jobs_task()
            }
            Message::OpenPrinterWebPage(web_page) => self.open_printer_web_page(web_page),
            Message::PrinterWebPageOpened(result) => {
                if let Err(why) = result {
                    tracing::warn!(why, "failed to open printer web page");
                }
                Task::none()
            }
        }
    }

    fn load_printer<M>(
        &mut self,
        printer: PrinterEntry,
        available_printers: Vec<PrinterEntry>,
    ) -> Task<M>
    where
        M: 'static + Send + From<Message> + From<crate::list::Message>,
    {
        self.printer = Some(printer);
        self.available_printers = available_printers;
        self.jobs.clear();
        self.clear_selection();
        self.error = None;
        self.action_error = None;
        self.show_completed = false;
        self.load_jobs_task()
    }

    fn apply_jobs_loaded<M>(
        &mut self,
        printer_id: String,
        result: Result<Vec<JobInfo>, String>,
    ) -> Task<M>
    where
        M: 'static + Send + From<Message> + From<crate::list::Message>,
    {
        if !self.is_current_printer(&printer_id) {
            return Task::none();
        }

        self.loading = false;
        self.operation_in_flight = false;

        let Ok(jobs) = result else {
            self.error = Some(fl!("failed-to-load-print-jobs"));
            return Task::none();
        };

        let active_job_count = if self.show_completed {
            jobs.iter()
                .filter(|job| {
                    matches!(
                        job.state,
                        JobState::Pending
                            | JobState::Processing
                            | JobState::Held
                            | JobState::Stopped
                            | JobState::Unknown
                    )
                })
                .count()
        } else {
            jobs.len()
        };

        self.selected_jobs
            .retain(|id| jobs.iter().any(|job| JobId::from_raw(job.id) == *id));

        self.jobs = jobs;
        self.error = None;

        cosmic::task::message(M::from(crate::list::Message::JobsLoaded {
            printer_id,
            result: Ok(active_job_count),
        }))
    }

    /// Sets the printer backend.
    pub fn set_backend(&mut self, backend: Backend) {
        self.backend = backend;
    }

    /// Returns whether a printer is loaded.
    #[must_use]
    pub fn has_printer(&self) -> bool {
        self.printer.is_some()
    }

    /// Clears the job selection and active menu.
    pub fn clear_selection(&mut self) {
        self.selected_jobs.clear();
        self.selection_anchor = None;
        self.menu = None;
    }

    // Right-click preserves an existing selection and does not apply keyboard modifiers.
    fn open_job_menu(&mut self, job_id: JobId) {
        if !self.selected_jobs.contains(&job_id) {
            self.selected_jobs.clear();
            self.selected_jobs.insert(job_id);
            self.selection_anchor = Some(job_id);
        }

        self.menu_position = self.cursor_position;
        self.menu = Some(QueueMenu::SelectedJobs);
    }

    fn open_whole_queue_menu(&mut self) {
        self.selected_jobs.clear();
        self.selection_anchor = None;
        self.menu_position = self.cursor_position;
        self.menu = Some(QueueMenu::Whole);
    }

    fn open_move_to_printer(&mut self, job_ids: Vec<JobId>) {
        if !job_ids.is_empty() {
            self.menu = Some(QueueMenu::MoveToPrinter { job_ids });
        }
    }

    fn start_job_action<M>(&mut self, operation: JobOperation) -> Task<M>
    where
        M: 'static + Send + From<Message> + From<crate::list::Message>,
    {
        self.menu = None;

        if self.operation_in_flight || !operation.is_available_for(&self.jobs) {
            return Task::none();
        }

        let Some(printer_id) = self
            .printer
            .as_ref()
            .map(|printer| printer.id().to_string())
        else {
            return Task::none();
        };

        self.operation_in_flight = true;
        let backend = self.backend.clone();

        cosmic::task::future(async move {
            let result = run_job_operation(backend, printer_id.clone(), operation).await;
            M::from(Message::JobActionFinished { printer_id, result })
        })
    }

    fn start_move_jobs<M>(&mut self, destination_id: String, job_ids: Vec<JobId>) -> Task<M>
    where
        M: 'static + Send + From<Message> + From<crate::list::Message>,
    {
        self.menu = None;

        if self.operation_in_flight || job_ids.is_empty() {
            return Task::none();
        }

        let Some(source_printer_id) = self
            .printer
            .as_ref()
            .map(|printer| printer.id().to_string())
        else {
            return Task::none();
        };

        if source_printer_id == destination_id
            || !self
                .available_printers
                .iter()
                .any(|printer| printer.id() == destination_id)
        {
            return Task::none();
        }

        self.operation_in_flight = true;

        let backend = self.backend.clone();

        cosmic::task::future(async move {
            let result =
                move_jobs(backend, source_printer_id.clone(), destination_id, job_ids).await;
            M::from(Message::JobActionFinished {
                printer_id: source_printer_id,
                result,
            })
        })
    }

    fn finish_job_action<M>(&mut self, printer_id: String, result: Result<(), String>) -> Task<M>
    where
        M: 'static + Send + From<Message> + From<crate::list::Message>,
    {
        if !self.is_current_printer(&printer_id) {
            return Task::none();
        }

        self.operation_in_flight = false;
        match result {
            Ok(()) => self.action_error = None,
            Err(why) => {
                tracing::warn!(printer_id, why, "print job operation failed");
                self.action_error = Some(why);
            }
        }

        self.load_jobs_task()
    }

    fn start_test_page<M>(&mut self) -> Task<M>
    where
        M: 'static + Send + From<Message> + From<crate::list::Message>,
    {
        self.menu = None;

        if self.operation_in_flight {
            return Task::none();
        }

        let Some(printer_id) = self
            .printer
            .as_ref()
            .map(|printer| printer.id().to_string())
        else {
            return Task::none();
        };

        // CUPS may need several seconds to create a queue before accepting the test page.
        self.operation_in_flight = true;
        let backend = self.backend.clone();

        cosmic::task::future(async move {
            let result = print_test_page(backend, printer_id.clone()).await;
            M::from(Message::TestPageFinished { printer_id, result })
        })
    }

    fn finish_test_page<M>(&mut self, printer_id: String, result: Result<i32, String>) -> Task<M>
    where
        M: 'static + Send + From<Message> + From<crate::list::Message>,
    {
        if !self.is_current_printer(&printer_id) {
            return Task::none();
        }

        self.operation_in_flight = false;
        match result {
            Ok(job_id) => {
                tracing::info!(printer_id, job_id, "queued a test page");
                self.action_error = None;
            }
            Err(why) => {
                tracing::warn!(printer_id, why, "failed to print a test page");
                self.action_error = Some(why);
                return Task::none();
            }
        }

        self.load_jobs_task()
    }

    fn open_printer_web_page<M>(&mut self, web_page: String) -> Task<M>
    where
        M: 'static + Send + From<Message> + From<crate::list::Message>,
    {
        self.menu = None;

        cosmic::task::future(async move {
            M::from(Message::PrinterWebPageOpened(
                backend::open_printer_web_page(web_page).await,
            ))
        })
    }

    fn is_current_printer(&self, printer_id: &str) -> bool {
        self.printer.as_ref().map(|printer| printer.id()) == Some(printer_id)
    }

    fn select_job(&mut self, job_id: JobId) {
        if self.modifiers.shift() {
            let Some(anchor) = self.selection_anchor else {
                self.selected_jobs.clear();
                self.selected_jobs.insert(job_id);
                self.selection_anchor = Some(job_id);
                return;
            };
            let Some(anchor_idx) = self
                .jobs
                .iter()
                .position(|job| JobId::from_raw(job.id) == anchor)
            else {
                return;
            };
            let Some(job_idx) = self
                .jobs
                .iter()
                .position(|job| JobId::from_raw(job.id) == job_id)
            else {
                return;
            };
            let (start, end) = if anchor_idx <= job_idx {
                (anchor_idx, job_idx)
            } else {
                (job_idx, anchor_idx)
            };
            if !self.modifiers.control() {
                self.selected_jobs.clear();
            }
            self.selected_jobs.extend(
                self.jobs[start..=end]
                    .iter()
                    .map(|job| JobId::from_raw(job.id)),
            );
        } else if self.modifiers.control() {
            if !self.selected_jobs.remove(&job_id) {
                self.selected_jobs.insert(job_id);
            }
            self.selection_anchor = Some(job_id);
        } else {
            self.selected_jobs.clear();
            self.selected_jobs.insert(job_id);
            self.selection_anchor = Some(job_id);
        }
    }

    fn load_jobs_task<M>(&mut self) -> Task<M>
    where
        M: 'static + Send + From<Message> + From<crate::list::Message>,
    {
        let Some(printer) = &self.printer else {
            return Task::none();
        };
        self.loading = true;
        let printer_id = printer.id().to_string();
        let filter = if self.show_completed {
            JobFilter::All
        } else {
            JobFilter::Active
        };
        let backend = self.backend.clone();

        cosmic::task::future(async move {
            let result = load_jobs(backend, printer_id.clone(), filter).await;
            M::from(Message::JobsLoaded { printer_id, result })
        })
    }
}

/// Returns the print queue view.
pub fn queue_view(state: &State) -> Element<'_, Message> {
    let body: Element<'_, Message> = if state.loading {
        queue_message(fl!("loading-print-jobs"))
    } else if let Some(error) = &state.error {
        queue_message(error.clone())
    } else if state.jobs.is_empty() {
        queue_message(fl!("no-jobs-waiting"))
    } else {
        queue_jobs(state)
    };

    let cancelable = job_ids_for_action(&state.jobs, JobAction::Cancel);
    let mut content = column::with_capacity(3)
        .spacing(24)
        .width(Length::Fill)
        .height(Length::Fill);

    if let Some(action_error) = &state.action_error {
        content = content.push(
            text::body(action_error.clone())
                .class(cosmic::theme::Text::Color(style::error()))
                .width(Length::Fill),
        );
    }

    content = content.push(body);

    if !cancelable.is_empty() {
        let cancel_all =
            JobOperation::new(JobAction::Cancel, cancelable).map(Message::RunJobAction);
        content = content.push(
            widget::button::standard(fl!("cancel-all"))
                .on_press_maybe((!state.operation_in_flight).then_some(cancel_all).flatten())
                .apply(container)
                .width(Length::Fill)
                .align_x(Alignment::End),
        );
    }

    let queue_surface = widget::mouse_area(
        container(content)
            .padding(QUEUE_CONTENT_PADDING)
            .width(Length::Fill)
            .height(Length::Fixed(QUEUE_SURFACE_HEIGHT)),
    )
    .on_move(Message::CursorMoved)
    .on_press(Message::ClearSelection)
    .on_right_press(Message::OpenWholeQueueMenu);

    let Some(menu) = &state.menu else {
        return queue_surface.into();
    };

    // `widget::context_menu` mispositions wide anchors and cannot nest job menus.
    widget::popover(queue_surface)
        .position(widget::popover::Position::Point(state.menu_position))
        .popup(match menu {
            QueueMenu::SelectedJobs => selected_jobs_menu(state),
            QueueMenu::Whole => whole_queue_menu(state),
            QueueMenu::MoveToPrinter { job_ids } => move_to_printer_menu(state, job_ids),
        })
        .on_close(Message::CloseMenu)
        .into()
}

fn queue_jobs(state: &State) -> Element<'_, Message> {
    let mut rows = column::with_capacity(state.jobs.len().saturating_mul(2));
    for (index, job) in state.jobs.iter().enumerate() {
        rows = rows.push(job_row(state, job));
        if index + 1 < state.jobs.len() {
            rows = rows.push(widget::divider::horizontal::default());
        }
    }

    scrollable(
        container(rows)
            .width(Length::Fill)
            .class(cosmic::theme::Container::List),
    )
    .width(Length::Fill)
    .height(Length::Shrink)
    .into()
}

fn job_row(state: &State, job: &JobInfo) -> Element<'static, Message> {
    let job_id = JobId::from_raw(job.id);
    let selected = state.selected_jobs.contains(&job_id);
    let content = row::with_capacity(2)
        .push(job_copy(job, selected))
        .push(job_controls(state, job, selected))
        .spacing(QUEUE_ROW_SPACING)
        .align_y(Alignment::Center)
        .padding(QUEUE_ROW_PADDING)
        .width(Length::Fill);

    let row = container(content).width(Length::Fill);
    let row = if selected {
        row.class(widgets::fill_container(style::selection(), 0.0))
    } else {
        row
    };

    // Prevent row presses from reaching the surface and clearing the new selection.
    cosmic::iced::widget::opaque(
        widget::mouse_area(row)
            .on_press(Message::SelectJob(job_id))
            .on_right_press(Message::OpenJobMenu(job_id)),
    )
}

fn job_copy(job: &JobInfo, selected: bool) -> Element<'static, Message> {
    let foreground = queue_row_foreground(selected);
    let title = text::body(if job.title.trim().is_empty() {
        fl!("untitled-print-job")
    } else {
        job.title.clone()
    })
    .size(14)
    .class(cosmic::theme::Text::Color(foreground))
    .wrapping(Wrapping::None)
    .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)))
    .width(Length::Fill);

    column::with_capacity(2)
        .push(title)
        .push(job_metadata(job, selected))
        .spacing(0)
        .width(Length::Fill)
        .into()
}

fn job_controls(state: &State, job: &JobInfo, selected: bool) -> Element<'static, Message> {
    let foreground = queue_row_foreground(selected);
    let mut controls = row::with_capacity(2)
        .width(Length::Fixed(QUEUE_CONTROLS_WIDTH))
        .height(Length::Fixed(32.0));

    let primary = match job.state {
        JobState::Processing => Some(("media-playback-pause-symbolic", JobAction::Pause)),
        JobState::Held | JobState::Stopped => {
            Some(("media-playback-start-symbolic", JobAction::Resume))
        }
        _ => None,
    };
    if let Some((icon, action)) = primary {
        let operation = JobOperation::single(action, JobId::from_raw(job.id));
        controls = controls.push(queue_icon_button(
            icon,
            foreground,
            (!state.operation_in_flight).then_some(Message::RunJobAction(operation)),
        ));
    } else if matches!(
        job.state,
        JobState::Pending | JobState::Aborted | JobState::Failed
    ) {
        // A true retry requires Restart-Job support from cosmic-printers.
        controls = controls.push(queue_icon_button(
            "view-refresh-symbolic",
            foreground,
            Some(Message::Refresh),
        ));
    } else {
        controls = controls.push(widget::space::horizontal().width(Length::Fixed(32.0)));
    }

    controls = controls.push(queue_icon_button(
        "window-close-symbolic",
        foreground,
        (JobAction::Cancel.is_available_for(&job.state) && !state.operation_in_flight).then_some(
            Message::RunJobAction(JobOperation::single(
                JobAction::Cancel,
                JobId::from_raw(job.id),
            )),
        ),
    ));

    controls.into()
}

fn queue_icon_button(
    name: &'static str,
    color: Color,
    message: Option<Message>,
) -> Element<'static, Message> {
    widget::button::icon(widget::icon::from_name(name))
        .class(cosmic::theme::Button::Custom {
            active: Box::new(move |_focused, theme| {
                let mut style = cosmic::widget::button::Catalog::active(
                    theme,
                    false,
                    false,
                    &cosmic::theme::Button::Icon,
                );
                style.icon_color = Some(color);
                style
            }),
            disabled: Box::new(|theme| {
                cosmic::widget::button::Catalog::disabled(theme, &cosmic::theme::Button::Icon)
            }),
            hovered: Box::new(|focused, theme| {
                cosmic::widget::button::Catalog::hovered(
                    theme,
                    focused,
                    false,
                    &cosmic::theme::Button::Icon,
                )
            }),
            pressed: Box::new(|focused, theme| {
                cosmic::widget::button::Catalog::pressed(
                    theme,
                    focused,
                    false,
                    &cosmic::theme::Button::Icon,
                )
            }),
        })
        .on_press_maybe(message)
        .into()
}

fn selected_jobs_menu(state: &State) -> Element<'static, Message> {
    let selected = state
        .jobs
        .iter()
        .filter(|job| state.selected_jobs.contains(&JobId::from_raw(job.id)))
        .collect::<Vec<_>>();
    let ids = selected
        .iter()
        .map(|job| JobId::from_raw(job.id))
        .collect::<Vec<_>>();
    let all_pause = !selected.is_empty()
        && selected
            .iter()
            .all(|job| JobAction::Pause.is_available_for(&job.state));
    let all_resume = !selected.is_empty()
        && selected
            .iter()
            .all(|job| JobAction::Resume.is_available_for(&job.state));
    let all_cancel = !selected.is_empty()
        && selected
            .iter()
            .all(|job| JobAction::Cancel.is_available_for(&job.state));
    let web_page = state
        .printer
        .as_ref()
        .and_then(|printer| printer.web_page().map(str::to_owned));

    menu_surface(
        column::with_capacity(7)
            .push(menu_row(
                fl!("cancel"),
                all_cancel
                    .then(|| JobOperation::new(JobAction::Cancel, ids.clone()))
                    .flatten()
                    .map(Message::RunJobAction),
            ))
            .push(menu_row(
                fl!("pause"),
                all_pause
                    .then(|| JobOperation::new(JobAction::Pause, ids.clone()))
                    .flatten()
                    .map(Message::RunJobAction),
            ))
            .push(menu_row(
                fl!("resume"),
                all_resume
                    .then(|| JobOperation::new(JobAction::Resume, ids.clone()))
                    .flatten()
                    .map(Message::RunJobAction),
            ))
            .push(menu_row(fl!("refresh"), Some(Message::Refresh)))
            .push(menu_divider())
            .push(move_to_printer_row(state, ids))
            .push(menu_divider())
            .push(menu_row(
                fl!("printer-web-interface"),
                web_page.map(Message::OpenPrinterWebPage),
            )),
    )
}

fn whole_queue_menu(state: &State) -> Element<'static, Message> {
    let cancelable = job_ids_for_action(&state.jobs, JobAction::Cancel);
    let pausable = job_ids_for_action(&state.jobs, JobAction::Pause);
    let resumable = job_ids_for_action(&state.jobs, JobAction::Resume);
    let web_page = state
        .printer
        .as_ref()
        .and_then(|printer| printer.web_page().map(str::to_owned));

    menu_surface(
        column::with_capacity(11)
            .push(menu_row(
                fl!("cancel-all"),
                JobOperation::new(JobAction::Cancel, cancelable).map(Message::RunJobAction),
            ))
            .push(menu_row(
                fl!("pause-all"),
                JobOperation::new(JobAction::Pause, pausable).map(Message::RunJobAction),
            ))
            .push(menu_row(
                fl!("resume-all"),
                JobOperation::new(JobAction::Resume, resumable).map(Message::RunJobAction),
            ))
            .push(menu_row(fl!("refresh-all"), Some(Message::Refresh)))
            .push(menu_divider())
            .push(menu_row(
                fl!("print-test-page"),
                (!state.operation_in_flight).then_some(Message::PrintTestPage),
            ))
            .push(menu_divider())
            .push(menu_toggle_row(
                fl!("show-completed-jobs"),
                state.show_completed,
                Some(Message::ToggleCompleted),
            ))
            .push(menu_divider())
            .push(move_to_printer_row(
                state,
                state
                    .jobs
                    .iter()
                    .filter(|job| job_can_move(&job.state))
                    .map(|job| JobId::from_raw(job.id))
                    .collect(),
            ))
            .push(menu_divider())
            .push(menu_row(
                fl!("printer-web-interface"),
                web_page.map(Message::OpenPrinterWebPage),
            )),
    )
}

fn move_to_printer_row(state: &State, job_ids: Vec<JobId>) -> Element<'static, Message> {
    let can_move = !state.operation_in_flight
        && !job_ids.is_empty()
        && state.available_printers.iter().any(|printer| {
            state
                .printer
                .as_ref()
                .is_none_or(|current| current.id() != printer.id())
        });

    menu_button(
        vec![
            text::body(fl!("move-to-printer"))
                .width(Length::Fill)
                .into(),
            widget::icon::from_name("go-next-symbolic")
                .size(style::ICON_SIZE)
                .into(),
        ],
        can_move.then_some(Message::OpenMoveToPrinter(job_ids)),
    )
}

fn move_to_printer_menu(state: &State, job_ids: &[JobId]) -> Element<'static, Message> {
    let current_printer_id = state.printer.as_ref().map(PrinterEntry::id);
    let mut destinations = column::with_capacity(state.available_printers.len());

    for printer in &state.available_printers {
        let message = (current_printer_id != Some(printer.id()) && !state.operation_in_flight)
            .then(|| Message::MoveJobs {
                destination_id: printer.id().to_string(),
                job_ids: job_ids.to_vec(),
            });
        let label = if printer.name().trim().is_empty() {
            printer.id().to_string()
        } else {
            printer.name().to_string()
        };

        destinations = destinations.push(menu_button(
            vec![
                text::body(label)
                    .wrapping(Wrapping::None)
                    .ellipsize(Ellipsize::End(EllipsizeHeightLimit::Lines(1)))
                    .width(Length::Fill)
                    .into(),
            ],
            message,
        ));
    }

    menu_surface(
        scrollable(destinations)
            .width(Length::Fill)
            .height(Length::Shrink)
            .apply(container)
            .max_height(QUEUE_DESTINATION_MENU_MAX_HEIGHT),
    )
}
fn menu_toggle_row(
    label: String,
    checked: bool,
    message: Option<Message>,
) -> Element<'static, Message> {
    let indicator: Element<'static, Message> = if checked {
        widget::icon::from_name("object-select-symbolic")
            .size(style::ICON_SIZE)
            .into()
    } else {
        widget::space::horizontal()
            .width(Length::Fixed(f32::from(style::ICON_SIZE)))
            .into()
    };

    menu_button(
        vec![indicator, text::body(label).width(Length::Fill).into()],
        message,
    )
}

fn menu_row(label: String, message: Option<Message>) -> Element<'static, Message> {
    menu_button(vec![text::body(label).width(Length::Fill).into()], message)
}

fn menu_button(
    children: Vec<Element<'static, Message>>,
    message: Option<Message>,
) -> Element<'static, Message> {
    menu::menu_button(children)
        .height(Length::Fixed(QUEUE_MENU_ROW_HEIGHT))
        .on_press_maybe(message)
        .into()
}

fn menu_divider() -> Element<'static, Message> {
    container(widget::divider::horizontal::light())
        .padding([0, cosmic::theme::active().cosmic().space_xxs()])
        .into()
}

fn menu_surface(content: impl Into<Element<'static, Message>>) -> Element<'static, Message> {
    container(content)
        .padding([cosmic::theme::active().cosmic().space_xxs(), 0])
        .width(Length::Fixed(QUEUE_MENU_WIDTH))
        .class(cosmic::theme::Container::Dropdown)
        .into()
}

fn queue_message(label: String) -> Element<'static, Message> {
    container(text::body(label))
        .center_x(Length::Fill)
        .width(Length::Fill)
        .into()
}

fn queue_row_foreground(selected: bool) -> Color {
    let theme = cosmic::theme::active();
    if selected {
        theme.cosmic().accent_color().into()
    } else {
        theme.cosmic().on_bg_color().into()
    }
}

fn job_metadata(job: &JobInfo, selected: bool) -> Element<'static, Message> {
    let foreground = queue_row_foreground(selected);
    let mut values = Vec::with_capacity(4);

    if !job.user.trim().is_empty() {
        values.push((job.user.clone(), foreground));
    }
    if job.size > 0 {
        values.push((format_job_size_k_octets(job.size), foreground));
    }
    if job.creation_time > 0 {
        values.push((
            relative_time_from_unix_timestamp(job.creation_time),
            foreground,
        ));
    }
    values.push((
        job_state_label(&job.state),
        job_state_color(&job.state, selected),
    ));

    let mut children = Vec::with_capacity(values.len().saturating_mul(2));
    for (index, (label, color)) in values.into_iter().enumerate() {
        if index > 0 {
            children.push(widgets::dot(foreground, 2.0));
        }
        children.push(
            text::caption(label)
                .class(cosmic::theme::Text::Color(color))
                .into(),
        );
    }

    widget::flex_row(children)
        .spacing(4)
        .align_items(Alignment::Center)
        .width(Length::Fill)
        .into()
}

fn format_job_size_k_octets(k_octets: i32) -> String {
    if k_octets >= 1024 {
        format!("{:.1} MB", f64::from(k_octets) / 1024.0)
    } else {
        format!("{k_octets} KB")
    }
}

fn relative_time_from_unix_timestamp(timestamp: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(timestamp);

    relative_time_from(now, timestamp)
}

fn relative_time_from(now: i64, timestamp: i64) -> String {
    let elapsed = now.saturating_sub(timestamp);
    if elapsed < 60 {
        fl!("job-time-just-now")
    } else if elapsed < 3600 {
        let count = elapsed / 60_i64;
        fl!("job-time-minutes", count = count)
    } else if elapsed < 86_400 {
        let count = elapsed / 3_600_i64;
        fl!("job-time-hours", count = count)
    } else {
        let count = elapsed / 86_400_i64;
        fl!("job-time-days", count = count)
    }
}

fn job_state_label(state: &JobState) -> String {
    match state {
        JobState::Pending => fl!("job-pending"),
        JobState::Processing => fl!("job-printing"),
        JobState::Completed => fl!("job-completed"),
        JobState::Canceled => fl!("job-canceled"),
        JobState::Aborted | JobState::Failed => fl!("job-error"),
        JobState::Held => fl!("job-paused"),
        JobState::Stopped => fl!("job-stopped"),
        JobState::Unknown => fl!("job-unknown"),
    }
}

fn job_ids_for_action(jobs: &[JobInfo], action: JobAction) -> Vec<JobId> {
    jobs.iter()
        .filter(|job| action.is_available_for(&job.state))
        .map(|job| JobId::from_raw(job.id))
        .collect()
}

async fn load_jobs(
    backend: Backend,
    printer_id: String,
    filter: JobFilter,
) -> Result<Vec<JobInfo>, String> {
    backend
        .jobs(&printer_id, filter)
        .await
        .map_err(|why| why.to_string())
}

async fn print_test_page(backend: Backend, printer_id: String) -> Result<i32, String> {
    backend
        .print_test_page(&printer_id)
        .await
        .map_err(|why| why.to_string())
}

async fn run_job_operation(
    backend: Backend,
    printer_id: String,
    operation: JobOperation,
) -> Result<(), String> {
    // Stop the batch at the first backend failure.
    for job_id in operation.job_ids {
        let job_id = job_id.into_raw();
        let result = match operation.action {
            JobAction::Pause => backend.pause_job(&printer_id, job_id).await,
            JobAction::Resume => backend.resume_job(&printer_id, job_id).await,
            JobAction::Cancel => backend.cancel_job(&printer_id, job_id).await,
        }
        .map_err(|why| why.to_string());
        result?;
    }
    Ok(())
}

async fn move_jobs(
    backend: Backend,
    source_printer_id: String,
    destination_printer_id: String,
    job_ids: Vec<JobId>,
) -> Result<(), String> {
    for job_id in job_ids {
        backend
            .move_job(
                &source_printer_id,
                job_id.into_raw(),
                &destination_printer_id,
            )
            .await
            .map_err(|why| why.to_string())?;
    }

    Ok(())
}

fn job_can_move(state: &JobState) -> bool {
    matches!(
        state,
        JobState::Pending | JobState::Held | JobState::Processing | JobState::Stopped
    )
}

fn job_state_color(state: &JobState, selected: bool) -> Color {
    match state {
        JobState::Processing => style::status_ready(),
        JobState::Aborted | JobState::Failed => style::error(),
        _ => queue_row_foreground(selected),
    }
}
