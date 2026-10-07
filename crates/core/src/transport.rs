use crate::Result;

/// The byte channel between a driver and its device.
///
/// Drivers only talk to the device through this trait, so they can be tested with
/// [`crate::testing::RecordingTransport`] and moved to another bus (USB bulk, serial, ...)
/// without touching their protocol code.
pub trait Transport: Send {
    /// Sends one output report. `report` is the payload without a report ID.
    fn write_report(&mut self, report: &[u8]) -> Result<()>;

    /// Reads an input report over the control pipe (HID `GET_REPORT`) into `buf`.
    /// Returns the number of bytes read, without the report ID.
    fn get_input_report(&mut self, report_id: u8, buf: &mut [u8]) -> Result<usize>;
}
