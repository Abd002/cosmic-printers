//! Standalone printers application.

use std::sync::OnceLock;

use cosmic::app::{Core, Settings, Task, context_drawer};
use cosmic::iced::widget::scrollable::{self as iced_scrollable, AbsoluteOffset};
use cosmic::iced::{Length, Subscription, window};
use cosmic::widget::{self, column, scrollable};
use cosmic::{ApplicationExt, Apply, Element};
use cosmic_printers_ui::{Backend, Request, add_printer, details, list, queue, strings};

// Keep one backend per process to avoid duplicate embedded discovery state.
static BACKEND: OnceLock<Backend> = OnceLock::new();

fn backend() -> Backend {
    BACKEND.get().cloned().unwrap_or_default()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Printers,
    Details,
}

#[derive(Clone, Debug)]
enum Message {
    List(list::Message),
    Details(details::Message),
    Queue(queue::Message),
    Request(Request),
    PrintersScrolled(AbsoluteOffset),
    CloseQueue,
}

impl From<list::Message> for Message {
    fn from(message: list::Message) -> Self {
        Self::List(message)
    }
}

impl From<details::Message> for Message {
    fn from(message: details::Message) -> Self {
        Self::Details(message)
    }
}

impl From<queue::Message> for Message {
    fn from(message: queue::Message) -> Self {
        Self::Queue(message)
    }
}

impl From<add_printer::Message> for Message {
    fn from(message: add_printer::Message) -> Self {
        Self::List(list::Message::AddPrinter(message))
    }
}

impl From<Request> for Message {
    fn from(request: Request) -> Self {
        Self::Request(request)
    }
}

// Section views borrow their titles, so keep them in application state.
struct Titles {
    printer_details: String,
    printer_information: String,
    printing_preferences: String,
    supplies: String,
    printer_queue: String,
}

impl Default for Titles {
    fn default() -> Self {
        Self {
            printer_details: strings::printer_details(),
            printer_information: strings::printer_information(),
            printing_preferences: strings::printing_preferences(),
            supplies: strings::supplies(),
            printer_queue: strings::printer_queue(),
        }
    }
}

struct App {
    core: Core,
    screen: Screen,
    printers_scroll_id: widget::Id,
    printers_scroll_offset: AbsoluteOffset,
    queue_open: bool,
    titles: Titles,
    list: list::State,
    details: details::State,
    queue: queue::State,
}

impl cosmic::Application for App {
    // CUPS work and DNS-SD discovery require the multi-thread executor.
    type Executor = cosmic::executor::multi::Executor;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "io.github.abd002.Printers";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: ()) -> (Self, Task<Message>) {
        let mut app = Self {
            core,
            screen: Screen::Printers,
            printers_scroll_id: widget::Id::unique(),
            printers_scroll_offset: AbsoluteOffset::default(),
            queue_open: false,
            titles: Titles::default(),
            list: list::State::default(),
            details: details::State::default(),
            queue: queue::State::default(),
        };

        let title_task = match app.core.main_window_id() {
            Some(id) => app.set_window_title("Printers".to_string(), id),
            None => Task::none(),
        };

        let backend = backend();
        app.list.set_dialog_application_id(Self::APP_ID);
        app.list.set_backend(backend.clone());
        app.details.set_backend(backend.clone());
        app.queue.set_backend(backend);

        let refresh_task = app.list.update(list::Message::Refresh);

        (app, Task::batch([title_task, refresh_task]))
    }

    fn header_start(&self) -> Vec<Element<'_, Message>> {
        let spacing = cosmic::theme::active().cosmic().spacing;

        let title = widget::row::with_capacity(2)
            .align_y(cosmic::iced::Alignment::Center)
            .spacing(spacing.space_xxs)
            .push(widget::icon::from_name("printer-symbolic").size(24).icon())
            .push(widget::text::heading("Printers"));

        vec![title.into()]
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::List(message) => self.list.update(message),
            Message::Details(message) => self.details.update(message),
            Message::Queue(message) => self.queue.update(message),
            Message::PrintersScrolled(offset) => {
                self.printers_scroll_offset = offset;
                Task::none()
            }
            Message::CloseQueue => {
                self.close_queue();
                Task::none()
            }

            Message::Request(Request::ShowDetails) => {
                self.screen = Screen::Details;
                Task::none()
            }
            Message::Request(Request::GoBack) => {
                self.screen = Screen::Printers;
                iced_scrollable::scroll_to(
                    self.printers_scroll_id.clone(),
                    self.printers_scroll_offset.into(),
                )
            }
            Message::Request(Request::ShowQueue) => {
                self.queue_open = true;
                self.core.window.show_context = true;
                Task::none()
            }
            Message::Request(Request::Surface(action)) => {
                cosmic::task::message(cosmic::Action::Cosmic(cosmic::app::Action::Surface(action)))
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::active().cosmic().spacing;

        let content = match self.screen {
            Screen::Printers => column::with_capacity(3)
                .spacing(spacing.space_l)
                .push(list::page_header().map(Message::List))
                .push(list::default_printer_view(&self.list, Message::from).map(Message::List))
                .push(list::printers_view(&self.list).map(Message::List)),
            Screen::Details => self.details_view(spacing.space_l),
        };

        let scrollable = content
            .padding([spacing.space_m, spacing.space_l])
            .apply(scrollable)
            .height(Length::Fill);

        match self.screen {
            Screen::Printers => scrollable
                .id(self.printers_scroll_id.clone())
                .on_scroll(|viewport| Message::PrintersScrolled(viewport.absolute_offset()))
                .into(),
            Screen::Details => scrollable.into(),
        }
    }

    fn view_window(&self, id: window::Id) -> Element<'_, Message> {
        self.list
            .add_printer_window(id)
            .map(|dialog| add_printer::dialog(dialog).map(Message::from))
            .unwrap_or_else(|| widget::space::horizontal().into())
    }

    fn context_drawer(&self) -> Option<context_drawer::ContextDrawer<'_, Message>> {
        if !self.queue_open || !self.queue.has_printer() {
            return None;
        }

        Some(
            context_drawer::context_drawer(
                queue::queue_view(&self.queue).map(Message::Queue),
                Message::CloseQueue,
            )
            .title(self.titles.printer_queue.clone()),
        )
    }

    fn on_escape(&mut self) -> Task<Message> {
        self.list
            .update(list::Message::AddPrinter(add_printer::Message::Close))
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            Subscription::run(printer_events).map(Message::List),
            window::close_events()
                .map(list::Message::AddPrinterDialogClosed)
                .map(Message::List),
        ])
    }
}

impl App {
    fn details_view(&self, spacing: u16) -> widget::Column<'_, Message> {
        if !self.details.has_printer() {
            return column::with_capacity(1)
                .push(details::nothing_selected_view().map(Message::Details));
        }

        let mut content = column::with_capacity(6).spacing(spacing);

        if let Some(header) = details::header_view(&self.details) {
            content = content.push(header.map(Message::Details));
        }

        content = content
            .push(
                details::default_and_queue_view(&self.details, &self.titles.printer_details)
                    .map(Message::Details),
            )
            .push(
                details::printer_information_view(&self.details, &self.titles.printer_information)
                    .map(Message::Details),
            )
            .push(
                details::printer_preferences_view(
                    &self.details,
                    &self.titles.printing_preferences,
                    Message::from,
                )
                .map(Message::Details),
            );

        if self.details.has_supplies() {
            content = content.push(
                details::supplies_view(&self.details, &self.titles.supplies).map(Message::Details),
            );
        }
        if self.details.can_remove_printer() {
            content =
                content.push(details::remove_printer_view(&self.details).map(Message::Details));
        }

        content
    }

    fn close_queue(&mut self) {
        self.queue_open = false;
        self.core.window.show_context = false;
        self.queue.clear_selection();
    }
}

fn printer_events() -> impl cosmic::iced::futures::Stream<Item = list::Message> {
    list::printer_events_subscription(backend())
}

fn main() -> cosmic::iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    cosmic_printers_ui::select_languages();

    let _ = BACKEND.set(Backend::detect_blocking());
    tracing::info!(backend = ?backend(), "serving printers");

    cosmic::app::run::<App>(
        Settings::default().size_limits(
            cosmic::iced::Limits::NONE
                .min_width(450.0)
                .min_height(300.0),
        ),
        (),
    )
}
