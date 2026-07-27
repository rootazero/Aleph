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
    s.replace('[', "`[")
        .replace('*', "`*")
        .replace('?', "`?")
}

pub struct WindowsPim;

impl WindowsPim {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    async fn run_powershell(&self, script: &str) -> Result<std::process::Output> {
        // `hidden_command`: without CREATE_NO_WINDOW a console child spawned by
        // the windowless daemon pops a black window on the user's screen for
        // every mail query.
        let output = aleph_desktop::script_exec::hidden_command("powershell.exe")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                script,
            ])
            .output()
            .await
            .map_err(|e| {
                DesktopError::PlatformError(format!(
                    "Failed to run PowerShell (Outlook integration requires PowerShell): {e}"
                ))
            })?;
        Ok(output)
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
        let script = format!(
            r#"
            try {{
                $outlook = New-Object -ComObject Outlook.Application
                $ns = $outlook.GetNamespace("MAPI")
                $targetFolder = $null
                foreach ($store in $ns.Folders) {{
                    $stack = New-Object System.Collections.Generic.Stack[object]
                    $stack.Push($store)
                    while ($stack.Count -gt 0) {{
                        $f = $stack.Pop()
                        if ($f.Name -eq "{folder_path}") {{
                            $targetFolder = $f
                            break
                        }}
                        foreach ($sub in $f.Folders) {{ $stack.Push($sub) }}
                    }}
                    if ($targetFolder) {{ break }}
                }}
                if (-not $targetFolder) {{
                    $targetFolder = $ns.GetDefaultFolder(6)
                }}
                $items = $targetFolder.Items
                $items.Sort("[ReceivedTime]", $true)
                $messages = @()
                $count = 0
                foreach ($item in $items) {{
                    if ($count -ge {limit}) {{ break }}
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
        let _ = WindowsPim::default();
    }
}
