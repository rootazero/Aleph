use aleph_desktop::pim_types::{MailFolder, MailMessage, MailMessageDetail};
use aleph_desktop::traits::PimCapability;
use aleph_desktop::{DesktopError, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct LinuxPim;

impl LinuxPim {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    fn thunderbird_dir() -> Option<PathBuf> {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|h| h.join(".thunderbird"))
    }

    fn find_mbox_files(&self) -> Vec<(String, PathBuf)> {
        let tb_dir = match Self::thunderbird_dir() {
            Some(d) if d.exists() => d,
            _ => return Vec::new(),
        };

        let mut result = Vec::new();
        let mail_root = tb_dir.join("Mail");
        if !mail_root.exists() {
            return result;
        }

        Self::walk_mail_dir(&mail_root, &mail_root, &mut result);
        result
    }

    fn walk_mail_dir(root: &Path, current: &Path, out: &mut Vec<(String, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(current) else {
            return;
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                Self::walk_mail_dir(root, &path, out);
            } else {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let name_lower = name.to_ascii_lowercase();
                if name_lower.starts_with('.')
                    || name_lower.ends_with(".msf")
                    || name_lower.ends_with(".dat")
                {
                    continue;
                }
                let relative = path.strip_prefix(root).unwrap_or(&path);
                let folder_id = relative.to_string_lossy().replace('\\', "/");
                out.push((folder_id, path));
            }
        }
    }

    fn parse_mbox(content: &str) -> Vec<ParsedMessage> {
        let mut messages = Vec::new();
        let mut start = 0usize;

        while start < content.len() {
            let next_from = Self::find_next_from_line(content, start + 1);
            let end = next_from.unwrap_or(content.len());
            let segment = &content[start..end];

            if let Some(msg) = Self::parse_message(segment, start) {
                messages.push(msg);
            }

            start = end;
        }

        messages
    }

    fn find_next_from_line(content: &str, from_pos: usize) -> Option<usize> {
        let bytes = content.as_bytes();
        let mut i = from_pos;
        // The probe reads up to `bytes[i + 5]`, so a separator in the last 6
        // bytes of the file is in range.
        while i + 5 < bytes.len() {
            if bytes[i] == b'\n'
                && bytes[i + 1] == b'F'
                && bytes[i + 2] == b'r'
                && bytes[i + 3] == b'o'
                && bytes[i + 4] == b'm'
                && bytes[i + 5] == b' '
            {
                return Some(i + 1);
            }
            i += 1;
        }
        None
    }

    fn parse_message(segment: &str, offset: usize) -> Option<ParsedMessage> {
        let trimmed = segment.trim_start();
        if trimmed.is_empty() {
            return None;
        }

        let mut lines = trimmed.lines();

        let first_line = lines.next()?;
        if !first_line.starts_with("From ") {
            if first_line.contains(':') {
                lines = trimmed.lines();
            } else {
                return None;
            }
        }

        let mut headers = HashMap::<String, String>::new();
        let mut body_lines = Vec::new();
        let mut in_body = false;
        let mut current_key: Option<String> = None;

        for line in lines {
            if in_body {
                body_lines.push(line);
                continue;
            }
            if line.is_empty() {
                in_body = true;
                continue;
            }
            if line.starts_with(' ') || line.starts_with('\t') {
                if let Some(ref key) = current_key {
                    if let Some(val) = headers.get_mut(key) {
                        val.push(' ');
                        val.push_str(line.trim());
                    }
                }
            } else if let Some((key, val)) = line.split_once(':') {
                let key_lc = key.trim().to_ascii_lowercase();
                headers.insert(key_lc.clone(), val.trim().to_string());
                current_key = Some(key_lc);
            }
        }

        let subject = headers.get("subject").cloned().unwrap_or_default();
        let from = headers.get("from").cloned().unwrap_or_default();
        let to = headers
            .get("to")
            .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
            .unwrap_or_default();
        let cc = headers
            .get("cc")
            .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
            .unwrap_or_default();
        let date = headers
            .get("date")
            .and_then(|d| DateTime::parse_from_rfc2822(d).ok())
            .map(|d| d.with_timezone(&Utc));

        let body = body_lines.join("\n");

        Some(ParsedMessage {
            subject,
            from,
            to,
            cc,
            date,
            body,
            offset,
        })
    }

    async fn read_mbox_file(path: &Path) -> Result<String> {
        tokio::fs::read_to_string(path).await.map_err(|e| {
            DesktopError::PlatformError(format!("Failed to read mbox {}: {e}", path.display()))
        })
    }
}

impl Default for LinuxPim {
    fn default() -> Self {
        Self::new()
    }
}

struct ParsedMessage {
    subject: String,
    from: String,
    to: Vec<String>,
    cc: Vec<String>,
    date: Option<DateTime<Utc>>,
    body: String,
    offset: usize,
}

#[async_trait]
impl PimCapability for LinuxPim {
    async fn mail_folders(&self) -> Result<Vec<MailFolder>> {
        let folders = self.find_mbox_files();
        let mut result = Vec::new();
        for (folder_id, path) in folders {
            let count = match tokio::fs::read_to_string(&path).await {
                Ok(content) => Self::parse_mbox(&content).len() as u32,
                Err(_) => 0,
            };
            result.push(MailFolder {
                id: folder_id.clone(),
                name: folder_id
                    .rsplit('/')
                    .next()
                    .unwrap_or(&folder_id)
                    .to_string(),
                count,
            });
        }
        Ok(result)
    }

    async fn mail_search(
        &self,
        query: &str,
        folder: Option<&str>,
        limit: u32,
    ) -> Result<Vec<MailMessage>> {
        let query_lc = query.to_ascii_lowercase();
        let folders = self.find_mbox_files();
        let mut result = Vec::new();

        let target_folders: Vec<(String, PathBuf)> = match folder {
            Some(f) => folders.into_iter().filter(|(id, _)| id == f).collect(),
            None => folders,
        };

        for (folder_id, path) in target_folders {
            let Ok(content) = Self::read_mbox_file(&path).await else {
                continue;
            };
            let messages = Self::parse_mbox(&content);
            for msg in messages {
                if result.len() >= limit as usize {
                    break;
                }
                let matches = msg.subject.to_ascii_lowercase().contains(&query_lc)
                    || msg.from.to_ascii_lowercase().contains(&query_lc)
                    || msg.body.to_ascii_lowercase().contains(&query_lc);
                if !matches {
                    continue;
                }
                result.push(MailMessage {
                    id: format!("{}:{}", folder_id, msg.offset),
                    subject: msg.subject.clone(),
                    sender: msg.from.clone(),
                    recipients: msg.to.clone(),
                    date: msg.date.unwrap_or_else(Utc::now),
                    body_preview: msg.body.chars().take(200).collect(),
                    is_read: true,
                });
            }
            if result.len() >= limit as usize {
                break;
            }
        }

        result.sort_by_key(|b| std::cmp::Reverse(b.date));
        Ok(result)
    }

    async fn mail_get(&self, message_id: &str) -> Result<MailMessageDetail> {
        // Split from the right: `folder_id` is an mbox relative path and may
        // itself contain `:` (legal in Linux file names); only the trailing
        // numeric offset is ours.
        let (folder_id, offset_str) = message_id.rsplit_once(':').ok_or_else(|| {
            DesktopError::PlatformError(format!("Invalid message_id format: {message_id}"))
        })?;
        let offset: usize = offset_str.parse().map_err(|_| {
            DesktopError::PlatformError(format!("Invalid offset in message_id: {message_id}"))
        })?;

        let folders = self.find_mbox_files();
        let path = folders
            .into_iter()
            .find(|(id, _)| id == folder_id)
            .map(|(_, p)| p)
            .ok_or_else(|| DesktopError::PlatformError(format!("Folder not found: {folder_id}")))?;

        let content = Self::read_mbox_file(&path).await?;
        let messages = Self::parse_mbox(&content);
        let msg = messages
            .into_iter()
            .find(|m| m.offset == offset)
            .ok_or_else(|| {
                DesktopError::PlatformError(format!("Message not found at offset {offset}"))
            })?;

        Ok(MailMessageDetail {
            id: message_id.to_string(),
            subject: msg.subject,
            sender: msg.from,
            recipients: msg.to,
            cc: msg.cc,
            bcc: Vec::new(),
            date: msg.date.unwrap_or_else(Utc::now),
            body: msg.body,
            is_read: true,
            attachments: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let _pim = LinuxPim;
    }

    #[test]
    fn find_from_line_separator_at_end_of_file() {
        // The "\nFrom " separator sits in the final 6 bytes of the content;
        // the scan must still reach it.
        let content = "From a@b Sat Jan  1 00:00:00 2022\nbody\nFrom ";
        let idx = LinuxPim::find_next_from_line(content, 1);
        assert_eq!(idx, Some(content.len() - 5));
    }

    #[tokio::test]
    async fn mail_get_folder_id_with_colon() {
        // `folder_id` is an mbox relative path and may itself contain ':'; the
        // split must happen at the trailing offset, not the folder's colon, so
        // the lookup reports the full folder id rather than a bad offset.
        let pim = LinuxPim::new();
        let err = pim.mail_get("Inbox:sub:42").await.unwrap_err();
        assert!(
            err.to_string().contains("Folder not found: Inbox:sub"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn mail_get_message_id_without_colon() {
        let pim = LinuxPim::new();
        let err = pim.mail_get("no-colon-here").await.unwrap_err();
        assert!(
            err.to_string().contains("Invalid message_id format"),
            "unexpected error: {err}"
        );
    }
}
