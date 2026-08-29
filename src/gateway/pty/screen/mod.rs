//! Server-side terminal screen state.
//!
//! The VT emulator lives here rather than in the client so that reconnect,
//! backpressure and multi-client screen sharing fall out of the architecture:
//! the server holds the screen, so `pty.attach` can hand a fresh client a full
//! snapshot, and what goes on the wire is a bounded per-frame diff instead of
//! an unbounded byte stream.

pub mod grid;
pub mod perform;
pub use grid::{Attrs, Cell, Color, Grid};
pub use perform::Screen;

#[cfg(test)]
mod tests {
    /// Pins the `vte` API surface this module is built on. If `vte` changes
    /// `Perform`'s method signatures or how `advance` is called, this test
    /// fails first and names the change, instead of every emulator test
    /// failing at once with a confusing error.
    ///
    /// Mutation-verified (2026-08-29): temporarily changed `execute`'s `byte`
    /// parameter from `u8` to `u32` (a plausible drifted signature) and
    /// rebuilt. `rustc` refused to compile with `E0053: method `execute` has
    /// an incompatible type for trait`, naming the exact method and the
    /// expected-vs-found types (`expected u8, found u32`). Confirms this
    /// guard catches a real signature drift, not just the crate being
    /// undeclared. Reverted after confirming.
    #[test]
    fn vte_perform_api_is_the_shape_this_module_assumes() {
        #[derive(Default)]
        struct Probe {
            printed: String,
            executed: Vec<u8>,
            csi: Vec<(Vec<u16>, char)>,
            osc: Vec<Vec<u8>>,
        }

        impl vte::Perform for Probe {
            fn print(&mut self, c: char) {
                self.printed.push(c);
            }
            fn execute(&mut self, byte: u8) {
                self.executed.push(byte);
            }
            fn csi_dispatch(
                &mut self,
                params: &vte::Params,
                _intermediates: &[u8],
                _ignore: bool,
                action: char,
            ) {
                let flat: Vec<u16> = params
                    .iter()
                    .map(|p| p.first().copied().unwrap_or(0))
                    .collect();
                self.csi.push((flat, action));
            }
            fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
                self.osc
                    .push(params.iter().flat_map(|p| p.to_vec()).collect());
            }
        }

        let mut parser = vte::Parser::new();
        let mut probe = Probe::default();
        parser.advance(&mut probe, b"hi\r\n\x1b[31m\x1b]0;title\x07");

        assert_eq!(probe.printed, "hi", "print() must receive printable chars");
        assert_eq!(
            probe.executed,
            vec![b'\r', b'\n'],
            "execute() must receive C0 controls"
        );
        assert_eq!(
            probe.csi,
            vec![(vec![31], 'm')],
            "csi_dispatch must receive SGR 31"
        );
        assert_eq!(probe.osc.len(), 1, "osc_dispatch must fire for OSC 0");
    }
}
