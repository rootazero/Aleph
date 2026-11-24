#[cfg(test)]
mod tests {
    use super::super::Screen;
    use aleph_protocol::pty::{PtyHistoryError, PtyHistoryRequest};

    fn request(before: Option<&str>, limit: u16) -> PtyHistoryRequest {
        PtyHistoryRequest {
            session_id: "test".into(),
            before: before.map(str::to_owned),
            limit,
        }
    }
    fn indices(page: &aleph_protocol::pty::PtyHistoryResponse) -> Vec<&str> {
        page.rows.iter().map(|row| row.index.as_str()).collect()
    }
    fn seeded() -> Screen {
        let mut screen = Screen::new(2, 4);
        screen.grid.set_scrollback_limit(3);
        for _ in 0..7 {
            screen.feed(b"x\r\n");
        }
        screen
    }

    #[test]
    fn history_cursor_survives_append_until_eviction() {
        let mut screen = seeded();
        assert_eq!(
            indices(&screen.history(&request(None, 32)).unwrap()),
            ["3", "4", "5"]
        );
        screen.feed(b"x\r\n");
        assert_eq!(
            indices(&screen.history(&request(Some("6"), 32)).unwrap()),
            ["4", "5"]
        );
        screen.feed(b"x\r\n");
        assert_eq!(
            indices(&screen.history(&request(Some("6"), 32)).unwrap()),
            ["5"]
        );
        let page = screen.history(&request(Some("4"), 32)).unwrap();
        assert!(page.rows.is_empty() && page.evicted);
        let page = screen.history(&request(Some("5"), 32)).unwrap();
        assert!(page.rows.is_empty() && !page.evicted);
    }

    #[test]
    fn history_page_size_is_bounded_and_does_not_skip_rows() {
        let screen = seeded();
        let before = screen.full_patch();
        let page = screen.history(&request(None, 32)).unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() <= 262_144);
        let one = screen.history(&request(None, 1)).unwrap();
        let exact = serde_json::to_vec(&one).unwrap().len();
        let bounded = screen
            .history_with_budget(&request(None, 32), exact)
            .unwrap();
        assert_eq!(bounded, one);
        let older = screen
            .history(&request(bounded.older_before.as_deref(), 32))
            .unwrap();
        assert_eq!(indices(&older), ["3", "4"]);
        assert_eq!(
            screen.history_with_budget(&request(None, 32), exact - 1),
            Err(PtyHistoryError::RowTooLarge)
        );
        for limit in [0, 65] {
            assert!(screen.history(&request(None, limit)).is_err());
        }
        for cursor in ["bad", "100", "18446744073709551616"] {
            assert!(screen.history(&request(Some(cursor), 32)).is_err());
        }
        assert_eq!(screen.full_patch(), before);
    }

    #[test]
    fn history_oversize_older_row_never_skips_to_smaller_rows() {
        let mut screen = Screen::new(1, 4);
        screen.feed(b"a\r\n");
        screen.resize(1, 1000);
        screen.feed(&vec![b'"'; 1000]);
        screen.feed(b"\r\n");
        screen.resize(1, 4);
        screen.feed(b"b\r\n");
        let page = screen.history_with_budget(&request(None, 64), 256).unwrap();
        assert_eq!(indices(&page), ["2"]);
        assert_eq!(page.older_before.as_deref(), Some("2"));
        assert_eq!(
            screen.history_with_budget(&request(Some("2"), 64), 256),
            Err(PtyHistoryError::RowTooLarge)
        );
        assert_eq!(
            indices(
                &screen
                    .history_with_budget(&request(Some("1"), 64), 256)
                    .unwrap()
            ),
            ["0"]
        );
    }

    #[test]
    fn history_dense_escaped_pages_and_large_indices_are_exact() {
        use super::super::{Attrs, Color};
        let mut screen = Screen::new(1, 1000);
        for _ in 0..8 {
            for col in 0..1000 {
                screen.grid.put(
                    if col % 2 == 0 { '"' } else { '\\' },
                    (
                        Color::Rgb(255, 128, (col % 255) as u8),
                        Color::Indexed(255),
                        Attrs(15),
                    ),
                );
            }
            screen.feed(b"\r\n");
        }
        // Exercise lossless indices, with no extra retained copy of history.
        screen.grid.history_next = u64::MAX;
        let mut cursor = None;
        let mut all = Vec::new();
        loop {
            let page = screen.history(&request(cursor.as_deref(), 64)).unwrap();
            assert!(serde_json::to_vec(&page).unwrap().len() <= 262_144);
            assert!(!page.rows.is_empty());
            all.extend(
                page.rows
                    .iter()
                    .map(|row| row.index.parse::<u64>().unwrap())
                    .rev(),
            );
            if page.older_before.is_none() {
                break;
            }
            if let Some(before) = cursor {
                assert!(
                    page.older_before.as_ref().unwrap().parse::<u64>().unwrap()
                        < before.parse::<u64>().unwrap()
                );
            }
            cursor = page.older_before;
        }
        assert_eq!(all, ((u64::MAX - 8)..u64::MAX).rev().collect::<Vec<_>>());
    }

    #[test]
    fn history_row_limit_and_retention_changes_preserve_indices() {
        let mut screen = Screen::new(1, 1);
        for _ in 0..100 {
            screen.feed(b"x\r\n");
        }
        let page = screen.history(&request(None, 64)).unwrap();
        assert_eq!(page.rows.len(), 64);
        assert_eq!(page.rows[0].index, "36");
        assert_eq!(page.older_before.as_deref(), Some("36"));
        screen.set_scrollback_limit(1);
        assert_eq!(
            indices(&screen.history(&request(None, 64)).unwrap()),
            ["99"]
        );
        screen.set_scrollback_limit(3);
        screen.feed(b"x\r\n");
        assert_eq!(
            indices(&screen.history(&request(None, 64)).unwrap()),
            ["99", "100"]
        );
        screen.resize(4, 4);
        screen.feed(b"\x1b[2;3r\x1b[20S");
        assert_eq!(
            indices(&screen.history(&request(None, 64)).unwrap()),
            ["99", "100"]
        );
    }

    #[test]
    fn history_primary_indices_survive_reset_resize_and_alternate() {
        let mut screen = seeded();
        let original = screen.history(&request(None, 32)).unwrap();
        screen.resize(3, 6);
        screen.feed(b"\x1b[2J\x1b[!p\x1bc");
        assert_eq!(screen.history(&request(None, 32)).unwrap(), original);
        screen.feed(b"\x1b[?1049hALT\r\n");
        assert_eq!(
            screen.history(&request(None, 32)),
            Err(PtyHistoryError::Unavailable)
        );
        screen.resize(4, 8);
        screen.feed(b"\x1b[?1049l");
        assert_eq!(screen.history(&request(None, 32)).unwrap(), original);
    }
}
