#![warn(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]
#![deny(unsafe_code)]

//! Server-side implementation of the COSMIC printers service.

mod context;
mod cups_backend;
mod dnssd;
mod error;
mod ipp;
mod printer_application_backend;
mod server;

pub use server::Server;
