//! Everything reached through libcups: what destinations exist, what each one says about itself,
//! and the operations that change one.

mod administration;
mod attributes;
mod authorization;
mod conversion;
mod destinations;
mod jobs;
mod refresh;
mod routing;
mod scheduler;
mod user_defaults;

pub(crate) use administration::{
    delete_printer, set_accept_jobs, set_enabled, set_info, set_location,
};
pub(crate) use authorization::{mark_administrable, may_administer_printers};
pub(crate) use jobs::{cancel_job, get_jobs, move_job, pause_job, print_test_page, resume_job};
pub(crate) use refresh::{refresh_available_destinations, reload_printer};
pub(crate) use user_defaults::{apply_saved, clear_default, set_default, set_option_default};
