use zlink::{ReplyError, introspect};

#[derive(Debug, PartialEq, ReplyError, introspect::ReplyError)]
#[zlink(interface = "com.system76.CosmicSettings.Printers")]
pub enum Error {
    FailedToGetPrinters,
    CupsFailed,
    PrinterNotFound,
    NoDefaultPrinter,
}
