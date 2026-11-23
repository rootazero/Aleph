// The Aleph Panel: one TSX source for the Desktop shell's embedded surface
// and the Web package served at `/v1/panel/`. The embedding host (Rust on
// Desktop, `web/host.js` on the Web) owns the Gateway connection.
import { mount } from "nsh";
import { Shell } from "./shell/Shell";

mount(Shell);
