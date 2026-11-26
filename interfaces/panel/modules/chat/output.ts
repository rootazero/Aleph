import { signal } from "nsh";
import type { PanelSdk } from "../../shell/sdk";

const MAX_OUTPUT_BYTES = 256 * 1024;
const PAGE_BYTES = 20 * 1024;

type Page = {
  tool_call_id: string;
  text: string;
  offset: number;
  total_bytes: number;
  truncated: boolean;
  source: "inline" | "persisted" | "expired";
};

export type OutputAddress = { run_id: string; tool_call_id: string };
export type OutputState = {
  text: string;
  next: number;
  total: number | null;
  loading: boolean;
  expired: boolean;
  limited: boolean;
  error: string | null;
};

function utf8Length(text: string): number {
  let bytes = 0;
  for (const char of text) {
    const code = char.codePointAt(0)!;
    bytes += code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
  }
  return bytes;
}

export function outputReader(sdk: PanelSdk, session: () => string | null, visible: () => boolean) {
  const state = signal<OutputState>({
    text: "", next: 0, total: null, loading: false, expired: false, limited: false, error: null,
  });
  let generation = 0;
  const reset = () => {
    ++generation;
    state.set({ text: "", next: 0, total: null, loading: false, expired: false, limited: false, error: null });
  };
  const load = async (address: OutputAddress): Promise<void> => {
    const sessionKey = session();
    const previous = state();
    if (!sessionKey || !visible() || previous.loading || previous.expired || previous.limited) return;
    const offset = previous.next;
    // Core may round the end forward by three bytes to finish a UTF-8 character.
    const limit = Math.min(PAGE_BYTES, MAX_OUTPUT_BYTES - offset - 3);
    if (limit <= 0 || (previous.total !== null && offset >= previous.total)) return;
    const thisRead = ++generation;
    const current = () => thisRead === generation && visible() && session() === sessionKey;
    state.set({ ...previous, loading: true, error: null });
    try {
      const page = await sdk.invoke("trace.tool_output", {
        session_key: sessionKey, run_id: address.run_id, tool_call_id: address.tool_call_id, offset, limit,
      }) as Page;
      if (!current()) return;
      if (page.tool_call_id !== address.tool_call_id || typeof page.text !== "string" ||
          !Number.isSafeInteger(page.total_bytes) || page.total_bytes < 0 ||
          (page.source !== "inline" && page.source !== "persisted" && page.source !== "expired")) {
        throw new Error("Aleph returned an invalid output page.");
      }
      if (page.source === "expired") {
        state.set({ text: "", next: 0, total: 0, loading: false, expired: true, limited: false, error: null });
        return;
      }
      if (offset > 0 && previous.total !== page.total_bytes) {
        reset();
        await load(address);
        return;
      }
      const bytes = utf8Length(page.text);
      const next = offset + bytes;
      if (page.offset !== offset || bytes > limit + 3 || next > page.total_bytes ||
          page.truncated !== (next < page.total_bytes) || (page.truncated && bytes === 0)) {
        throw new Error("Aleph returned an incomplete output page.");
      }
      state.set({ text: previous.text + page.text, next, total: page.total_bytes,
        loading: false, expired: false, limited: page.truncated && MAX_OUTPUT_BYTES - next <= 3, error: null });
    } catch (failure) {
      if (!current()) return;
      state.set({ ...previous, loading: false, error: (failure as Error).message || "Tool output unavailable." });
    }
  };
  return { state, load, reset };
}
