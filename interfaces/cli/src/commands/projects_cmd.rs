//! `aleph projects` — the headless face of project rooms.
//!
//! Every other operator-only family (`users`, `audit`, `spend`) already had a
//! CLI and rooms did not, which matters because a headless deployment has no
//! Panel at all. `channel bind` / `channel unbind` are admin-gated
//! server-side; the CLI reaches the server over loopback, which resolves to
//! the implicit owner as `"operator"` — the same posture that put those three
//! here rather than only in the Panel. `channel list` is open to a room's
//! members.
//!
//! ## No shape is declared here
//!
//! Every request and every row is a type from `aleph_protocol::projects`,
//! which the server **constructs** its responses from. That direction is what
//! makes the column reconciliation below mean something: a test that parses a
//! response proves the client's fields are a *subset* of what was sent, never
//! that the two are the same set. Three families shipped the other way round —
//! `aleph workspace create`, the TUI's `agent.run`, and `aleph providers
//! list/get/add`, the last of which rendered two columns
//! (`type` / `default`) the server had never sent, so every row printed a dash
//! from the day it was written. A dash reads as "no value yet", not as a bug.

use aleph_protocol::projects::{
    BindingPeerKind, ChannelBindParams, ChannelBindResult, ChannelBindingRow, ChannelListParams,
    ChannelListResult, ChannelUnbindParams, ChannelUnbindResult, ProjectListResult, ProjectRow,
    RescopeOutcome, UNBIND_KEEPS_TRANSCRIPT_NOTICE,
};
use serde_json::Value;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliError, CliResult};

/// (display header, wire field name) for every column `projects list` renders,
/// in print order.
///
/// The wire half is not decorative — see this module's doc, and
/// `every_room_column_is_backed_by_a_real_wire_field` below.
const ROOM_COLUMNS: &[(&str, &str)] = &[
    ("ID", "id"),
    ("Name", "name"),
    ("Status", "status"),
    ("Owner", "owner_user_id"),
    ("Members", "member_ids"),
    ("Workspace", "workspace_path"),
];

/// Fields of a [`ProjectRow`] that `projects list` deliberately does not print.
///
/// Named rather than merely absent, because the reverse reconciliation asserts
/// this set EXACTLY: a field the server grows and this table silently drops
/// looks, on screen, identical to there being nothing more to show. Adding a
/// field to the wire therefore has to arrive here or in [`ROOM_COLUMNS`], and
/// saying which is a decision rather than an omission.
///
/// These three are timestamps. A room list is read to find an id to act on,
/// and three date columns would push the id and name off the left of a narrow
/// terminal to answer a question nobody asked it.
#[cfg(test)]
const ROOM_FIELDS_NOT_RENDERED: &[&str] = &["created_at", "updated_at", "last_used_at"];

/// (display header, wire field name) for every column `channel list` renders.
const BINDING_COLUMNS: &[(&str, &str)] = &[
    ("Channel", "channel_id"),
    ("Kind", "peer_kind"),
    ("Conversation", "peer_id"),
    ("Label", "label"),
    ("Bound By", "bound_by"),
    ("Bound At", "bound_at"),
];

/// Fields of a [`ChannelBindingRow`] that `channel list` does not print.
///
/// One entry, and it is the command's own argument: every row in this table
/// belongs to the room the user just named, so a column repeating it would be
/// the same string on every line.
#[cfg(test)]
const BINDING_FIELDS_NOT_RENDERED: &[&str] = &["project_id"];

/// A cell for an absent optional. One spelling, so "the server sent null" and
/// "this client cannot read that field" at least look the same everywhere
/// rather than differing by call site.
const ABSENT: &str = "-";

/// One `projects list` row's cells, in [`ROOM_COLUMNS`] order.
///
/// Split out from the command body for the reason `spend_cmd::render_row`
/// documents: while the cells were an inline `vec![]`, nothing could observe
/// that they agreed with the headers in length and order.
/// `output::print_table` takes its column count from the headers alone, so a
/// cell too few shifts every value one column left of its title and a cell too
/// many is dropped — and the table still renders, which is worse than a dash.
fn render_room(row: &ProjectRow) -> Vec<String> {
    vec![
        row.id.clone(),
        row.name.clone(),
        row.status.clone(),
        row.owner_user_id.clone().unwrap_or_else(|| ABSENT.into()),
        row.member_ids.len().to_string(),
        row.workspace_path.clone().unwrap_or_else(|| ABSENT.into()),
    ]
}

/// One `channel list` row's cells, in [`BINDING_COLUMNS`] order.
fn render_binding(row: &ChannelBindingRow) -> Vec<String> {
    vec![
        row.channel_id.clone(),
        // `Display` for the peer kind, which is the same word serde puts on
        // the wire (pinned by `every_peer_kind_spells_one_word_everywhere` in
        // the protocol crate). Not a hand-written match: a table that prints
        // one spelling while `bind` accepts another is how an operator learns
        // to type the wrong thing.
        row.peer_kind.to_string(),
        row.peer_id.clone(),
        row.label.clone().unwrap_or_else(|| ABSENT.into()),
        row.bound_by.clone().unwrap_or_else(|| ABSENT.into()),
        format_secs(row.bound_at),
    ]
}

/// Render a stored epoch-**second** timestamp as UTC.
///
/// Seconds, not milliseconds: `ChannelBinding::bound_at` is written by
/// `ProjectStore`'s `now_secs()`. The unit is spelled out here rather than
/// inferred at the call site because the repo carries both conventions —
/// `spend`'s ledger is milliseconds and `audit`'s is seconds, and reading one
/// as the other silently renders 1970 or the year 56000.
fn format_secs(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0).map_or_else(
        || secs.to_string(),
        |t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
    )
}

/// Parse a `--peer-kind` argument, before any connection is opened.
///
/// The conversion goes through `aleph_protocol`'s `FromStr`, which accepts
/// exactly the spellings the server's deserializer accepts and nothing else.
/// Hand-rolling a `match` here — or accepting `"Group"` case-insensitively and
/// normalizing it — would put a fourth author on a two-word vocabulary whose
/// three previous authors are the reason Ruling AF exists. It would also make
/// the CLI accept a spelling the JSON API rejects, which is the more expensive
/// half: `peer_kind` is part of the binding's primary key, so a value that
/// slips through mints a row nothing ever matches.
///
/// Failing locally also means a typo costs a message rather than a round trip
/// that comes back as a bare `INVALID_PARAMS` — the shape `aleph providers
/// add` shipped with, where every invocation had always failed.
fn parse_peer_kind(raw: &str) -> CliResult<BindingPeerKind> {
    raw.parse::<BindingPeerKind>()
        .map_err(|e| CliError::Other(e.to_string()))
}

/// `aleph projects list`
pub async fn list(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let raw: Value = client
        .call("projects.list", Some(serde_json::json!({})))
        .await?;
    client.close().await?;

    let result: ProjectListResult =
        serde_json::from_value(raw.clone()).map_err(|e| CliError::Other(e.to_string()))?;

    let headers: Vec<&str> = ROOM_COLUMNS.iter().map(|(header, _)| *header).collect();
    let rows: Vec<Vec<String>> = result.projects.iter().map(render_room).collect();
    output::print_table(&headers, &rows, json, &raw);
    Ok(())
}

/// `aleph projects channel list <project_id>`
pub async fn channel_list(
    server_url: &str,
    config: &CliConfig,
    project_id: &str,
    json: bool,
) -> CliResult<()> {
    let params = ChannelListParams {
        project_id: project_id.to_string(),
    };

    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let raw: Value = client.call("projects.channel.list", Some(params)).await?;
    client.close().await?;

    let result: ChannelListResult =
        serde_json::from_value(raw.clone()).map_err(|e| CliError::Other(e.to_string()))?;

    let headers: Vec<&str> = BINDING_COLUMNS.iter().map(|(header, _)| *header).collect();
    let rows: Vec<Vec<String>> = result.bindings.iter().map(render_binding).collect();
    output::print_table(&headers, &rows, json, &raw);
    Ok(())
}

/// The conversation `channel bind` is about to name.
///
/// Five borrowed strings behind one parameter rather than five: the command
/// already took eight arguments, and these five are exactly the fields
/// `ChannelBindParams` carries. Destructured on the first line, so the body
/// reads as it did before.
pub struct BindSpec<'a> {
    pub project_id: &'a str,
    pub channel_id: &'a str,
    pub peer_id: &'a str,
    /// Still a `&str` here, because this is argv. It becomes a
    /// [`BindingPeerKind`] in [`parse_peer_kind`], before anything connects.
    pub peer_kind: &'a str,
    pub label: Option<&'a str>,
}

/// `aleph projects channel bind …` (operator only)
pub async fn channel_bind(
    server_url: &str,
    config: &CliConfig,
    spec: &BindSpec<'_>,
    json: bool,
) -> CliResult<()> {
    let BindSpec {
        project_id,
        channel_id,
        peer_id,
        peer_kind,
        label,
    } = *spec;
    // Before connecting: a rejected spelling should cost a sentence, not a
    // round trip that returns a bare protocol error.
    let params = ChannelBindParams {
        project_id: project_id.to_string(),
        channel_id: channel_id.to_string(),
        peer_kind: parse_peer_kind(peer_kind)?,
        peer_id: peer_id.to_string(),
        label: label.map(str::to_string),
    };

    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let raw: Value = client.call("projects.channel.bind", Some(params)).await?;
    client.close().await?;

    if json {
        output::print_json(&raw);
        return Ok(());
    }

    let result: ChannelBindResult =
        serde_json::from_value(raw).map_err(|e| CliError::Other(e.to_string()))?;
    print_lines(&bind_receipt(&result));
    Ok(())
}

/// `aleph projects channel unbind …` (operator only)
pub async fn channel_unbind(
    server_url: &str,
    config: &CliConfig,
    channel_id: &str,
    peer_id: &str,
    peer_kind: &str,
    json: bool,
) -> CliResult<()> {
    let params = ChannelUnbindParams {
        channel_id: channel_id.to_string(),
        peer_kind: parse_peer_kind(peer_kind)?,
        peer_id: peer_id.to_string(),
    };

    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let raw: Value = client.call("projects.channel.unbind", Some(params)).await?;
    client.close().await?;

    if json {
        output::print_json(&raw);
        return Ok(());
    }

    let result: ChannelUnbindResult =
        serde_json::from_value(raw).map_err(|e| CliError::Other(e.to_string()))?;
    print_lines(&unbind_receipt(channel_id, peer_id, &result));
    Ok(())
}

/// The sentence for one rescope outcome.
///
/// `users_cmd::update` once printed a hardcoded "devices revoked" line and
/// said it whether or not a single device existed. A client asserting a result
/// it did not observe is more expensive than a client that says nothing, which
/// is why these are three different sentences and none of them is a default.
///
/// A `const fn` over the enum rather than three `println!` arms, so the
/// receipt's wording can be compared by a test without capturing stdout — and
/// so the three sentences have exactly one author, which is the property the
/// test is checking.
const fn rescope_sentence(outcome: RescopeOutcome) -> &'static str {
    // Exhaustive: a fourth outcome must be a compile error here rather than an
    // arm that silently reuses a neighbour's copy.
    match outcome {
        RescopeOutcome::Moved => {
            "The conversation's existing transcript now belongs to the room and is \
             visible to its roster."
        }
        // NOT "nobody has spoken in that conversation yet". That is an
        // INTERPRETATION of what the server reported, and the server reported
        // "I found no session row" — an interpretation is a factual claim this
        // command cannot support.
        RescopeOutcome::NothingToMove => {
            "No existing transcript was moved — no session was found for that conversation."
        }
        // Deliberately not the sentence above. `NothingToMove` is "the store
        // answered, and the answer was none"; `Unknown` is "the store did not
        // answer". Rendering the second as the first is this client inventing
        // a result the server never gave it.
        RescopeOutcome::Unknown => {
            "The binding is recorded, but whether an existing transcript moved could not \
             be determined — the session store did not answer. Check `aleph doctor`, then \
             run the same bind again to retry the move: re-binding a conversation to the \
             project it is already bound to is idempotent, and the handler retries the \
             rescope every time."
        }
    }
}

/// What the bind changed, and what it did not.
fn bind_receipt(result: &ChannelBindResult) -> Vec<String> {
    vec![
        format!(
            "Bound {}:{} to {}.",
            result.binding.channel_id, result.binding.peer_id, result.binding.project_id
        ),
        rescope_sentence(result.rescoped_session).to_string(),
    ]
}

/// What `unbind` did, and — the part a reader will otherwise assume wrong —
/// what it did not.
///
/// The notice is imported, never typed: it must be byte-identical here, in the
/// Panel, and in the server-side doc, and copy with three authors is this
/// repo's most-recorded defect. Precedent: `ADMIN_REQUIRED_MESSAGE`.
///
/// It is included only when something was actually released. On
/// `unbound: false` there is no transcript decision to explain, and saying it
/// anyway would describe an event that did not happen.
fn unbind_receipt(channel_id: &str, peer_id: &str, result: &ChannelUnbindResult) -> Vec<String> {
    if result.unbound {
        vec![
            format!("Released {channel_id}:{peer_id}."),
            UNBIND_KEEPS_TRANSCRIPT_NOTICE.to_string(),
        ]
    } else {
        vec![format!(
            "Nothing to release: {channel_id}:{peer_id} was not bound to any room."
        )]
    }
}

fn print_lines(lines: &[String]) {
    for line in lines {
        println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn sample_room() -> ProjectRow {
        ProjectRow {
            id: "p-1".into(),
            name: "eng".into(),
            owner_user_id: Some("u-owner".into()),
            workspace_path: Some("/srv/eng".into()),
            status: "active".into(),
            member_ids: vec!["u-owner".into(), "u-bob".into()],
            created_at: 0,
            updated_at: 0,
            last_used_at: 0,
        }
    }

    fn sample_binding() -> ChannelBindingRow {
        ChannelBindingRow {
            project_id: "p-1".into(),
            channel_id: "telegram".into(),
            // Ruling V: a typed `BindingPeerKind`, not a string. `"group".into()`
            // does not compile — do not "fix" that by loosening the contract type.
            peer_kind: BindingPeerKind::Group,
            peer_id: "c1".into(),
            bound_by: Some("u-owner".into()),
            bound_at: 1_704_067_200,
            label: Some("#eng".into()),
        }
    }

    /// Owned keys, not borrowed: every caller passes a temporary `Value`, and
    /// borrowing from it needs a `let` binding at each of the four sites.
    fn wire_keys(v: &Value) -> BTreeSet<String> {
        v.as_object()
            .expect("a row serialises to a JSON object")
            .keys()
            .cloned()
            .collect()
    }

    /// Every column this command renders must be backed by a field the server
    /// actually sends. Serialises the CONTRACT type — the one the server
    /// constructs its response from — rather than a literal copied from a
    /// plan, so a rename on either side of the wire shows up here instead of
    /// as a silent dash in the rendered table.
    #[test]
    fn every_room_column_is_backed_by_a_real_wire_field() {
        let keys = wire_keys(&serde_json::to_value(sample_room()).unwrap());
        for (header, field) in ROOM_COLUMNS {
            assert!(
                keys.contains(*field),
                "column {header:?} renders wire key {field:?}, which ProjectRow does not \
                 have. A header backed by nothing prints a dash forever, and a dash reads \
                 as missing data rather than as a bug."
            );
        }
    }

    #[test]
    fn every_binding_column_is_backed_by_a_real_wire_field() {
        let keys = wire_keys(&serde_json::to_value(sample_binding()).unwrap());
        for (header, field) in BINDING_COLUMNS {
            assert!(
                keys.contains(*field),
                "column {header:?} renders wire key {field:?}, which ChannelBindingRow does \
                 not have."
            );
        }
    }

    /// The direction the forward test is structurally blind to: a field the
    /// server sends that no column renders.
    ///
    /// The forward test catches a column pointing at a key that does not exist
    /// — a dash in every row, which at least looks wrong. This one catches the
    /// server growing a field the CLI never learned to print, whose symptom is
    /// that the table looks complete. "There is nothing more to show" and
    /// "this client has not caught up" render identically.
    ///
    /// Asserted as set EQUALITY against a NAMED exclusion list rather than as
    /// a subset, because a subset assertion is what lets an unrendered field
    /// through in the first place.
    #[test]
    fn every_room_field_is_either_a_column_or_a_named_exclusion() {
        let sent = wire_keys(&serde_json::to_value(sample_room()).unwrap());
        let mut accounted: BTreeSet<String> =
            ROOM_COLUMNS.iter().map(|(_, f)| (*f).to_string()).collect();
        accounted.extend(ROOM_FIELDS_NOT_RENDERED.iter().map(|f| (*f).to_string()));
        assert_eq!(
            sent, accounted,
            "ProjectRow's wire fields must each be either a column or listed in \
             ROOM_FIELDS_NOT_RENDERED with a reason. A field in neither is data the \
             server sends and this table silently drops."
        );
    }

    #[test]
    fn every_binding_field_is_either_a_column_or_a_named_exclusion() {
        let sent = wire_keys(&serde_json::to_value(sample_binding()).unwrap());
        let mut accounted: BTreeSet<String> =
            BINDING_COLUMNS.iter().map(|(_, f)| (*f).to_string()).collect();
        accounted.extend(BINDING_FIELDS_NOT_RENDERED.iter().map(|f| (*f).to_string()));
        assert_eq!(
            sent, accounted,
            "ChannelBindingRow's wire fields must each be either a column or listed in \
             BINDING_FIELDS_NOT_RENDERED with a reason."
        );
    }

    /// Headers and cells are two hand-maintained lists that must agree in
    /// length and order, and nothing in the type system says so.
    /// `output::print_table` takes its column count from the headers alone, so
    /// a mismatch does not error — it shifts.
    #[test]
    fn each_render_emits_one_cell_per_column() {
        assert_eq!(
            render_room(&sample_room()).len(),
            ROOM_COLUMNS.len(),
            "render_room and ROOM_COLUMNS disagree; print_table would shift or drop"
        );
        assert_eq!(
            render_binding(&sample_binding()).len(),
            BINDING_COLUMNS.len(),
            "render_binding and BINDING_COLUMNS disagree"
        );
    }

    /// The request the CLI sends must be exactly the shape the handler parses
    /// — not a superset it tolerates and not a subset it rejects.
    ///
    /// `aleph providers add` sent a flat `{name, type, api_key, base_url}` to
    /// a handler that wanted `{name, config: {…}}`, so every invocation of
    /// that command had always answered `INVALID_PARAMS`.
    #[test]
    fn the_bind_request_carries_exactly_what_the_handler_requires() {
        let v = serde_json::to_value(ChannelBindParams {
            project_id: "p-1".into(),
            channel_id: "telegram".into(),
            peer_kind: BindingPeerKind::Group,
            peer_id: "c1".into(),
            label: None,
        })
        .unwrap();
        assert_eq!(
            wire_keys(&v),
            ["channel_id", "peer_id", "peer_kind", "project_id"]
                .into_iter()
                .map(str::to_string)
                .collect::<BTreeSet<_>>(),
            "an omitted label must be omitted, and every other key must be present"
        );
    }

    #[test]
    fn the_unbind_request_carries_exactly_what_the_handler_requires() {
        let v = serde_json::to_value(ChannelUnbindParams {
            channel_id: "telegram".into(),
            peer_kind: BindingPeerKind::Thread,
            peer_id: "c1".into(),
        })
        .unwrap();
        assert_eq!(
            wire_keys(&v),
            ["channel_id", "peer_id", "peer_kind"]
                .into_iter()
                .map(str::to_string)
                .collect::<BTreeSet<_>>()
        );
    }

    /// `--peer-kind` must admit exactly the wire's spellings.
    ///
    /// The rejection half is the one that matters: a CLI that accepted
    /// `"Group"` and normalized it would work, and would leave the CLI
    /// accepting a spelling the JSON API rejects — an inconsistency somebody
    /// eventually "fixes" in whichever direction is easier.
    #[test]
    fn peer_kind_accepts_the_wire_spellings_and_only_those() {
        for kind in BindingPeerKind::ALL {
            assert_eq!(
                parse_peer_kind(kind.as_str()).expect("the wire spelling parses"),
                kind
            );
        }
        for bad in ["Group", "GROUP", "groups", "", "dm"] {
            let err = parse_peer_kind(bad)
                .expect_err("only the wire spellings are accepted")
                .to_string();
            for kind in BindingPeerKind::ALL {
                assert!(
                    err.contains(kind.as_str()),
                    "rejecting {bad:?} must name the accepted spellings: {err}"
                );
            }
        }
    }

    /// The receipt for a bind must say something DIFFERENT for each of the
    /// three outcomes, and in particular must not describe `Unknown` — "the
    /// store did not answer" — using `NothingToMove`'s words, which assert
    /// that it did.
    ///
    /// Asserted on the rendered sentences rather than by reading the source,
    /// because two arms sharing a sentence is a copy-paste away and compiles
    /// perfectly.
    #[test]
    fn the_three_rescope_outcomes_read_as_three_different_answers() {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for outcome in [
            RescopeOutcome::Moved,
            RescopeOutcome::NothingToMove,
            RescopeOutcome::Unknown,
        ] {
            // Exhaustiveness tripwire: a fourth variant is a compile error
            // here, in the same file as the sentences.
            match outcome {
                RescopeOutcome::Moved
                | RescopeOutcome::NothingToMove
                | RescopeOutcome::Unknown => {}
            }
            assert!(
                seen.insert(rescope_sentence(outcome).to_string()),
                "{outcome:?} reuses another outcome's wording; the client would be \
                 reporting an answer the server did not give"
            );
        }
        assert!(
            !rescope_sentence(RescopeOutcome::Unknown).contains("no session was found"),
            "`Unknown` means the store did not answer — claiming it found nothing is a \
             fact this client never received"
        );
        assert!(
            !rescope_sentence(RescopeOutcome::NothingToMove).contains("nobody has spoken"),
            "the server reported \"no row found\"; \"nobody has spoken\" is an inference \
             layered on top of it"
        );
    }

    /// The unbind notice must be the shared constant, not a local retype.
    ///
    /// Byte equality against `aleph_protocol`'s constant is the whole
    /// assertion: this sentence has to be identical on the CLI, in the Panel
    /// and in the server-side doc, and three copies means three authors.
    #[test]
    fn the_unbind_notice_is_the_shared_constant() {
        let released = unbind_receipt(
            "telegram",
            "c1",
            &ChannelUnbindResult { unbound: true },
        );
        assert!(
            released.contains(&UNBIND_KEEPS_TRANSCRIPT_NOTICE.to_string()),
            "the receipt must carry the shared constant verbatim, not a retype: {released:?}"
        );
        assert!(
            UNBIND_KEEPS_TRANSCRIPT_NOTICE.contains("does not move"),
            "the notice must still say the history does NOT come back — that is the \
             assumption it exists to correct"
        );
    }

    /// A no-op unbind must not narrate a transcript decision that did not
    /// happen.
    ///
    /// `unbound: false` means nothing was bound, so there was no history to
    /// leave anywhere. Printing the notice regardless would be the same class
    /// of defect as `users_cmd::update`'s unconditional "devices revoked" —
    /// copy that describes an event on a run where the event did not occur.
    #[test]
    fn a_noop_unbind_says_nothing_happened_and_explains_nothing_else() {
        let nothing = unbind_receipt(
            "telegram",
            "c1",
            &ChannelUnbindResult { unbound: false },
        );
        assert!(
            !nothing
                .iter()
                .any(|line| line.contains(UNBIND_KEEPS_TRANSCRIPT_NOTICE)),
            "nothing was released, so there is no transcript decision to explain: {nothing:?}"
        );
        assert!(
            nothing.iter().any(|l| l.contains("was not bound")),
            "the receipt must say plainly that there was nothing to release: {nothing:?}"
        );
    }

    /// The bind receipt must actually carry the outcome sentence — the
    /// three-way wording above is only worth testing if the receipt uses it.
    #[test]
    fn the_bind_receipt_carries_the_outcome_sentence() {
        for outcome in [
            RescopeOutcome::Moved,
            RescopeOutcome::NothingToMove,
            RescopeOutcome::Unknown,
        ] {
            let lines = bind_receipt(&ChannelBindResult {
                binding: sample_binding(),
                rescoped_session: outcome,
            });
            assert!(
                lines.contains(&rescope_sentence(outcome).to_string()),
                "{outcome:?}: the receipt must state the outcome, not just the binding: \
                 {lines:?}"
            );
            assert!(
                lines.iter().any(|l| l.contains("telegram") && l.contains("p-1")),
                "{outcome:?}: the receipt must name what was bound to what: {lines:?}"
            );
        }
    }
}
