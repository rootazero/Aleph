use aleph_desktop::pim_types::{MailAttachment, MailFolder, MailMessage, MailMessageDetail};
use aleph_desktop::traits::PimCapability;
use aleph_desktop::{DesktopError, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};

/// Escape a value for safe interpolation into a PowerShell double-quoted string.
/// Neutralizes the backtick escape char first, then `$(...)`/`$var` subexpression
/// expansion, then the closing quote — preventing command injection from
/// LLM/tool-supplied folder names, search queries, and message ids.
fn ps_escape_dq(s: &str) -> String {
    s.replace('`', "``").replace('$', "`$").replace('"', "\"\"")
}

/// Escape PowerShell wildcard characters so a user-originated query placed
/// inside `-like "*{query}*"` or `-Filter` is matched literally rather than
/// treated as a wildcard pattern.
fn escape_powershell_wildcards(s: &str) -> String {
    s.replace('[', "`[").replace('*', "`*").replace('?', "`?")
}

/// Escape a value for a DASL string literal (`'…'`), where the only special
/// character is the quote itself, doubled.
fn escape_dasl(s: &str) -> String {
    s.replace('\'', "''")
}

/// Build the `Items.Restrict` query that makes Outlook do the searching.
///
/// The previous implementation walked **every item** in the folder and read
/// `.Body` off each one — a separate MAPI round trip per message, against
/// mailboxes that routinely hold tens of thousands. It did not merely take a
/// long time; on any real Inbox it could not finish at all.
///
/// `Restrict` pushes the same three predicates into the message store, which
/// answers them from its own indexes. The `-like` comparison in the loop is kept
/// afterwards so the result set is *identical* whether the restriction was
/// applied or the fallback scan ran — a provider that rejects the query (some
/// PST/IMAP stores refuse `textdescription`) then costs correctness nothing.
fn restrict_query(query: &str) -> String {
    let q = escape_dasl(query);
    format!(
        "@SQL=\"urn:schemas:httpmail:subject\" LIKE '%{q}%' \
         OR \"urn:schemas:httpmail:fromname\" LIKE '%{q}%' \
         OR \"urn:schemas:httpmail:textdescription\" LIKE '%{q}%'"
    )
}

/// Hard ceiling on how many items the fallback scan touches.
///
/// Reached only when `Restrict` was refused. Without it the loop is unbounded in
/// the one situation where each iteration is most expensive, and the tool's only
/// protection is the process timeout — which returns nothing at all, rather than
/// the newest matches it had already found.
const MAX_SCAN_ITEMS: u32 = 5_000;

pub struct WindowsPim;

impl WindowsPim {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Run an Outlook COM script under the shared script deadline.
    ///
    /// It used to be a bare `.output()`, which made this the **last capture path
    /// in the workspace without a timeout** — and the one most likely to need
    /// it. `New-Object -ComObject Outlook.Application` does not fail fast when
    /// Outlook is unhappy: a first-run profile wizard, a "choose profile"
    /// dialog, a password prompt or a stuck send/receive all leave the COM call
    /// waiting for a window nobody is looking at, forever. The turn then hung
    /// until the harness's own ceiling with an orphaned `powershell.exe` behind
    /// it.
    async fn run_powershell(&self, script: &str) -> Result<std::process::Output> {
        use aleph_desktop::script_exec::{hidden_command, output_capped, RUN_SCRIPT_TIMEOUT};

        // `hidden_command`: without CREATE_NO_WINDOW a console child spawned by
        // the windowless daemon pops a black window on the user's screen for
        // every mail query.
        let mut cmd = hidden_command("powershell.exe");
        cmd.args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ]);
        output_capped(cmd, RUN_SCRIPT_TIMEOUT).await.map_err(|e| {
            DesktopError::PlatformError(format!(
                "Outlook integration via PowerShell failed: {e}. Outlook must be installed and \
                 able to open without a prompt (no profile chooser, no password dialog)."
            ))
        })
    }
}

impl Default for WindowsPim {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PimCapability for WindowsPim {
    async fn mail_folders(&self) -> Result<Vec<MailFolder>> {
        let script = r#"
            try {
                $outlook = New-Object -ComObject Outlook.Application
                $ns = $outlook.GetNamespace("MAPI")
                $script:folders = @()
                function Get-Folders($folder, $path) {
                    $fullPath = if ($path) { "$path\$($folder.Name)" } else { $folder.Name }
                    $count = $folder.Items.Count
                    # Use $script: scope: PowerShell `+=` inside a function reads
                    # the enclosing var but assigns to a new function-local copy,
                    # so a plain `$folders +=` discarded every append on return.
                    $script:folders += [PSCustomObject]@{id=$fullPath;name=$folder.Name;count=$count}
                    foreach ($sub in $folder.Folders) {
                        Get-Folders $sub $fullPath
                    }
                }
                foreach ($store in $ns.Folders) {
                    Get-Folders $store ""
                }
                $script:folders | ConvertTo-Json -Compress
            } catch {
                Write-Error "Outlook error: $_"
                exit 1
            }
        "#;

        let output = self.run_powershell(script).await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::PlatformError(format!(
                "Outlook mail_folders failed: {}",
                stderr.trim()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            return Ok(Vec::new());
        }

        let folders: Vec<serde_json::Value> = serde_json::from_str(&stdout)
            .or_else(|_| {
                let single: serde_json::Value = serde_json::from_str(&stdout)?;
                Ok::<_, serde_json::Error>(vec![single])
            })
            .map_err(|e| {
                DesktopError::PlatformError(format!("Failed to parse Outlook folders: {e}"))
            })?;

        let result = folders
            .into_iter()
            .filter_map(|v| {
                Some(MailFolder {
                    id: v.get("id")?.as_str()?.to_string(),
                    name: v.get("name")?.as_str()?.to_string(),
                    count: v.get("count")?.as_u64()? as u32,
                })
            })
            .collect();

        Ok(result)
    }

    async fn mail_search(
        &self,
        query: &str,
        folder: Option<&str>,
        limit: u32,
    ) -> Result<Vec<MailMessage>> {
        let folder_path = ps_escape_dq(folder.unwrap_or("Inbox"));
        let escaped_query = escape_powershell_wildcards(&ps_escape_dq(query));
        let restrict = ps_escape_dq(&restrict_query(query));
        let max_scan = MAX_SCAN_ITEMS;
        let script = format!(
            r#"
            try {{
                $outlook = New-Object -ComObject Outlook.Application
                $ns = $outlook.GetNamespace("MAPI")
                $targetFolder = $null
                # Match either the leaf name or the full "Store\Path\Leaf" id.
                # `mail_folders` returns the full path as each folder's `id`, so
                # matching only on `Name` meant handing this tool the id its own
                # sibling produced silently fell through to the default Inbox —
                # the caller got results, from the wrong folder, with no signal.
                foreach ($store in $ns.Folders) {{
                    $stack = New-Object System.Collections.Generic.Stack[object]
                    $stack.Push([PSCustomObject]@{{ F = $store; P = $store.Name }})
                    while ($stack.Count -gt 0) {{
                        $node = $stack.Pop()
                        if ($node.F.Name -eq "{folder_path}" -or $node.P -eq "{folder_path}") {{
                            $targetFolder = $node.F
                            break
                        }}
                        foreach ($sub in $node.F.Folders) {{
                            $stack.Push([PSCustomObject]@{{ F = $sub; P = "$($node.P)\$($sub.Name)" }})
                        }}
                    }}
                    if ($targetFolder) {{ break }}
                }}
                if (-not $targetFolder) {{
                    $targetFolder = $ns.GetDefaultFolder(6)
                }}
                # Let the store do the searching. A provider that refuses the
                # query (some PST / IMAP stores have no full-text index) falls
                # back to the bounded scan below; the -like test after it makes
                # both paths return the same set.
                $items = $targetFolder.Items
                $candidates = $null
                try {{ $candidates = $items.Restrict("{restrict}") }} catch {{ $candidates = $null }}
                if ($null -eq $candidates) {{ $candidates = $items }}
                try {{ $candidates.Sort("[ReceivedTime]", $true) }} catch {{ }}
                $messages = @()
                $count = 0
                $scanned = 0
                foreach ($item in $candidates) {{
                    if ($count -ge {limit}) {{ break }}
                    $scanned++
                    if ($scanned -gt {max_scan}) {{ break }}
                    # Coalesce nulls: some item types (meeting requests, receipts)
                    # have a null Body, and calling .Substring on it below would
                    # throw into the outer catch and abort the ENTIRE search on the
                    # first such match. Subject/SenderName can be null too and would
                    # break deserialization on the Rust side.
                    $subject = if ($null -eq $item.Subject) {{ "" }} else {{ $item.Subject }}
                    $sender = if ($null -eq $item.SenderName) {{ "" }} else {{ $item.SenderName }}
                    $body = if ($null -eq $item.Body) {{ "" }} else {{ $item.Body }}
                    if ($subject -like "*{escaped_query}*" -or $sender -like "*{escaped_query}*" -or $body -like "*{escaped_query}*") {{
                        $date = $item.ReceivedTime.ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
                        $messages += [PSCustomObject]@{{
                            id=$item.EntryID
                            subject=$subject
                            sender=$sender
                            recipients=@($item.To)
                            date=$date
                            body_preview=$body.Substring(0, [Math]::Min(200, $body.Length))
                            is_read=$item.UnRead -eq $false
                        }}
                        $count++
                    }}
                }}
                $messages | ConvertTo-Json -Compress
            }} catch {{
                Write-Error "Outlook error: $_"
                exit 1
            }}
            "#
        );

        let output = self.run_powershell(&script).await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::PlatformError(format!(
                "Outlook mail_search failed: {}",
                stderr.trim()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() || stdout == "null" {
            return Ok(Vec::new());
        }

        let messages: Vec<serde_json::Value> = serde_json::from_str(&stdout)
            .or_else(|_| {
                let single: serde_json::Value = serde_json::from_str(&stdout)?;
                Ok::<_, serde_json::Error>(vec![single])
            })
            .map_err(|e| {
                DesktopError::PlatformError(format!("Failed to parse Outlook messages: {e}"))
            })?;

        let result = messages
            .into_iter()
            .filter_map(|v| {
                let date_str = v.get("date")?.as_str()?;
                let date = DateTime::parse_from_rfc3339(date_str)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))?;

                let recipients = v
                    .get("recipients")?
                    .as_array()?
                    .iter()
                    .filter_map(|r| r.as_str().map(std::string::ToString::to_string))
                    .collect();

                Some(MailMessage {
                    id: v.get("id")?.as_str()?.to_string(),
                    subject: v
                        .get("subject")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string(),
                    sender: v
                        .get("sender")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string(),
                    recipients,
                    date,
                    body_preview: v.get("body_preview")?.as_str().unwrap_or("").to_string(),
                    is_read: v.get("is_read")?.as_bool().unwrap_or(true),
                })
            })
            .collect();

        Ok(result)
    }

    async fn mail_get(&self, message_id: &str) -> Result<MailMessageDetail> {
        let escaped_id = ps_escape_dq(message_id);
        let script = format!(
            r#"
            try {{
                $outlook = New-Object -ComObject Outlook.Application
                $ns = $outlook.GetNamespace("MAPI")
                $item = $ns.GetItemFromID("{escaped_id}")
                $date = $item.ReceivedTime.ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
                $recipients = @($item.To -split ';' | ForEach-Object {{ $_.Trim() }})
                $cc = @($item.CC -split ';' | ForEach-Object {{ $_.Trim() }})
                $attachments = @()
                foreach ($att in $item.Attachments) {{
                    $attachments += [PSCustomObject]@{{
                        filename=$att.FileName
                        mime_type="application/octet-stream"
                        size=$att.Size
                    }}
                }}
                [PSCustomObject]@{{
                    id=$item.EntryID
                    subject=$item.Subject
                    sender=$item.SenderName
                    recipients=$recipients
                    cc=$cc
                    bcc=@()
                    date=$date
                    body=$item.Body
                    is_read=$item.UnRead -eq $false
                    attachments=$attachments
                }} | ConvertTo-Json -Compress
            }} catch {{
                Write-Error "Outlook error: $_"
                exit 1
            }}
            "#
        );

        let output = self.run_powershell(&script).await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::PlatformError(format!(
                "Outlook mail_get failed: {}",
                stderr.trim()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let v: serde_json::Value = serde_json::from_str(&stdout).map_err(|e| {
            DesktopError::PlatformError(format!("Failed to parse Outlook message detail: {e}"))
        })?;

        let date_str = v
            .get("date")
            .and_then(|d| d.as_str())
            .unwrap_or("1970-01-01T00:00:00Z");
        let date = DateTime::parse_from_rfc3339(date_str).ok().map_or_else(
            || Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now),
            |d| d.with_timezone(&Utc),
        );

        let recipients = v
            .get("recipients")
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|r| r.as_str().map(std::string::ToString::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let cc = v
            .get("cc")
            .and_then(|r| r.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|r| r.as_str().map(std::string::ToString::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let attachments = v
            .get("attachments")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|att| {
                        Some(MailAttachment {
                            filename: att.get("filename")?.as_str()?.to_string(),
                            mime_type: att
                                .get("mime_type")?
                                .as_str()
                                .unwrap_or("application/octet-stream")
                                .to_string(),
                            size: att.get("size")?.as_u64().unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(MailMessageDetail {
            id: message_id.to_string(),
            subject: v
                .get("subject")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            sender: v
                .get("sender")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            recipients,
            cc,
            bcc: Vec::new(),
            date,
            body: v
                .get("body")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            is_read: v
                .get("is_read")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
            attachments,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let _ = WindowsPim;
    }

    #[test]
    fn dasl_literals_escape_only_the_quote() {
        assert_eq!(escape_dasl("O'Brien"), "O''Brien");
        // A DASL string literal has no other metacharacter; leaving the rest
        // alone keeps the query matching what the caller typed.
        assert_eq!(escape_dasl(r#"a"b`c$d"#), r#"a"b`c$d"#);
    }

    #[test]
    fn the_restrict_query_covers_the_same_three_fields_the_loop_tests() {
        // If these ever drift, the restricted path and the fallback scan return
        // different result sets for the same call — the worst kind of
        // difference, because which one ran depends on the message store.
        let q = restrict_query("invoice");
        assert!(q.starts_with("@SQL="));
        assert!(q.contains("urn:schemas:httpmail:subject"));
        assert!(q.contains("urn:schemas:httpmail:fromname"));
        assert!(q.contains("urn:schemas:httpmail:textdescription"));
        assert_eq!(q.matches("LIKE '%invoice%'").count(), 3);
    }

    #[test]
    fn a_quoted_query_cannot_break_out_of_the_dasl_literal() {
        let q = restrict_query("it's");
        assert!(q.contains("LIKE '%it''s%'"), "got {q}");
    }
}
