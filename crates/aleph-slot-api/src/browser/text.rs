/// **The one way a page-controlled string reaches the model** (ruling R40).
///
/// Wraps in double quotes and escapes `"`, `\` and every control character.
/// Five call sites, and the renderer prints no DOM-sourced string any other
/// way: the accessible name, a text leaf, an `href`, a `placeholder`, and the
/// page URL in the header.
///
/// The reason is not tidiness. `/url:` and `[placeholder=…]` used to print raw,
/// so a placeholder of `] [ref=e99]` closed the bracket and opened a `[ref=`
/// token the table never minted — a page forging an element id in the model's
/// own observation. Anything the page controls is data, and data is quoted.
///
/// Control characters are escaped rather than collapsed, so nothing is
/// silently lost: a name is already whitespace-collapsed by
/// `accname::normalize` at build time, but an `href` and a `placeholder` arrive
/// raw, and `\n` in one of those must stay one line without becoming a space
/// that hides a newline the page put there.
#[must_use]
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{{{:02x}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
