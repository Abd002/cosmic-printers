//! The printers application model.

use cosmic::app::{Core, Task, context_drawer};
use cosmic::iced::widget::scrollable::{self as iced_scrollable, AbsoluteOffset};
use cosmic::iced::{Length, Subscription};
use cosmic::widget::{self, column, scrollable};
use cosmic::{ApplicationExt, Apply, Element};
use cosmic_printers_ui::{Request, add_printer, details, list, queue, strings};

use crate::{backend, printer_events};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Printers,
    Details,
}

#[derive(Clone, Debug)]
pub enum Message {
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

pub struct App {
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
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let spacing = cosmic::theme::active().cosmic().spacing;

        let content = match self.screen {
            Screen::Printers => column::with_capacity(3)
                .spacing(spacing.space_l)
                .push(list::page_header().map(Message::List))
                .push(list::default_printer_view(&self.list).map(Message::List))
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
                .on_scroll(|viewport: iced_scrollable::Viewport| {
                    Message::PrintersScrolled(viewport.absolute_offset())
                })
                .into(),
            Screen::Details => scrollable.into(),
        }
    }

    fn dialog(&self) -> Option<Element<'_, Message>> {
        self.list
            .add_printer_dialog()
            .map(|dialog| add_printer::dialog(dialog).map(Message::from))
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
        Subscription::run(printer_events).map(Message::List)
    }
}

impl App {
    fn details_view(&self, spacing: u16) -> widget::Column<'_, Message, cosmic::Theme> {
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
                details::printer_preferences_view(&self.details, &self.titles.printing_preferences)
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
