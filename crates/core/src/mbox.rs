//! mbox files: public-inbox's exports, lei's output, Thunderbird's folders.

use mail_parser::mailbox::mbox::MessageIterator;

/// The raw bytes of each message in an mbox.
///
/// Read as mboxrd (what public-inbox, lei and b4 write): a line starting
/// "From " separates messages, and ">From ", ">>From " lose one ">". An
/// mboxo file (older Thunderbird) has no way to tell a quoted ">From " from
/// an escaped one, so such a line in it loses a ">" it should keep.
///
/// The blank line a writer puts after each message, before the next
/// "From " line, is not part of the message (Python's mailbox agrees), so a
/// message reads the same from an mbox as from its own file.
pub fn split(raw: &[u8]) -> Vec<Vec<u8>> {
    MessageIterator::new(raw)
        .filter_map(Result::ok)
        .map(|m| {
            let mut contents = m.unwrap_contents();
            if contents.ends_with(b"\n\n") {
                contents.pop();
            } else if contents.ends_with(b"\r\n\r\n") {
                contents.truncate(contents.len() - 2);
            }
            contents
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_and_quoted_from_lines() {
        let raw = b"junk before\nFrom a@x Mon Oct  5 10:00:00 2026\nMessage-ID: <1@x>\n\n>From here\n\
                    From b@x Mon Oct  5 10:00:01 2026\nMessage-ID: <2@x>\n\n>>From there\n";
        let msgs = split(raw);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0], b"Message-ID: <1@x>\n\nFrom here\n");
        assert_eq!(msgs[1], b"Message-ID: <2@x>\n\n>From there\n");
        assert!(split(b"").is_empty());
        // The separating blank line goes; a message's own last line stays.
        let raw = b"From a@x Mon Oct  5 10:00:00 2026\nMessage-ID: <1@x>\n\nbody\n\nFrom b@x Mon Oct  5 10:00:01 2026\nMessage-ID: <2@x>\n\nlast\n";
        assert_eq!(split(raw), vec![b"Message-ID: <1@x>\n\nbody\n".to_vec(), b"Message-ID: <2@x>\n\nlast\n".to_vec()]);
    }
}
