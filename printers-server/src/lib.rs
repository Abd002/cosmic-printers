#![warn(missing_docs)]
#![warn(rustdoc::broken_intra_doc_links)]
#![deny(unsafe_code)]

//! Server-side implementation of the COSMIC printers service.

mod cups;
mod dnssd;
mod error;
mod ipp;
mod printer_app;
mod server;
mod state;

pub use server::Server;
