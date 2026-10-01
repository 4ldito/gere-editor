//! PTY-backed terminal sessions. Parsing and PTY I/O stay off the UI thread.
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::{
    io::{Read, Write},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
};

pub(super) const SCROLLBACK_LINES: usize = 10_000;

pub(super) struct Terminal {
    pub(super) title: String,
    pub(super) screen: Arc<Mutex<vt100::Parser>>,
    pub(super) revision: Arc<AtomicU64>,
    pub(super) closed: Arc<AtomicBool>,
    pub(super) exited: bool,
    pub(super) size: (u16, u16),
    master: Box<dyn MasterPty + Send>,
    child: Option<Box<dyn portable_pty::Child + Send>>,
    input: mpsc::Sender<Vec<u8>>,
}

impl Terminal {
    pub(super) fn new(root: &Path, title: String, size: (u16, u16)) -> Result<Self, String> {
        let pair = native_pty_system()
            .openpty(pty_size(size))
            .map_err(|error| error.to_string())?;
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into());
        let mut command = CommandBuilder::new(shell);
        command.cwd(root);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| error.to_string())?;
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| error.to_string())?;
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|error| error.to_string())?;
        let screen = Arc::new(Mutex::new(vt100::Parser::new(
            size.0,
            size.1,
            SCROLLBACK_LINES,
        )));
        let revision = Arc::new(AtomicU64::new(0));
        let closed = Arc::new(AtomicBool::new(false));
        let (input, outgoing) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            while let Ok(bytes) = outgoing.recv() {
                if writer.write_all(&bytes).is_err() {
                    break;
                }
            }
        });
        let read_screen = screen.clone();
        let read_revision = revision.clone();
        let read_closed = closed.clone();
        std::thread::spawn(move || {
            let mut bytes = [0; 8192];
            while let Ok(count) = reader.read(&mut bytes) {
                if count == 0 {
                    break;
                }
                if let Ok(mut parser) = read_screen.lock() {
                    parser.process(&bytes[..count]);
                    read_revision.fetch_add(1, Ordering::Release);
                }
            }
            read_closed.store(true, Ordering::Release);
        });
        Ok(Self {
            title,
            screen,
            revision,
            closed,
            exited: false,
            size,
            master: pair.master,
            child: Some(child),
            input,
        })
    }

    pub(super) fn send(&self, bytes: impl Into<Vec<u8>>) {
        if !self.exited {
            let _ = self.input.send(bytes.into());
        }
    }

    pub(super) fn resize(&mut self, size: (u16, u16)) {
        if self.size == size {
            return;
        }
        if self.master.resize(pty_size(size)).is_ok() {
            if let Ok(mut parser) = self.screen.lock() {
                parser.screen_mut().set_size(size.0, size.1);
            }
            self.size = size;
            self.revision.fetch_add(1, Ordering::Release);
        }
    }

    pub(super) fn check_exit(&mut self) -> bool {
        if self.exited || !self.closed.load(Ordering::Acquire) {
            return false;
        }
        if let Some(child) = &mut self.child {
            if matches!(child.try_wait(), Ok(Some(_))) {
                self.exited = true;
                self.child = None;
                return true;
            }
        }
        false
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            std::thread::spawn(move || {
                let _ = child.kill();
                let _ = child.wait();
            });
        }
    }
}

fn pty_size((rows, cols): (u16, u16)) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

pub(super) fn key_bytes(key: &gpui::Keystroke, application_cursor: bool) -> Option<Vec<u8>> {
    let modifiers = key.modifiers;
    if modifiers.platform || modifiers.alt && modifiers.control {
        return None;
    }
    let sequence = match key.key.as_str() {
        "enter" => b"\r".as_slice(),
        "backspace" => b"\x7f",
        "tab" if modifiers.shift => b"\x1b[Z",
        "tab" => b"\t",
        "escape" => b"\x1b",
        "up" if application_cursor => b"\x1bOA",
        "down" if application_cursor => b"\x1bOB",
        "right" if application_cursor => b"\x1bOC",
        "left" if application_cursor => b"\x1bOD",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "delete" => b"\x1b[3~",
        "insert" => b"\x1b[2~",
        "pageup" => b"\x1b[5~",
        "pagedown" => b"\x1b[6~",
        "f1" => b"\x1bOP",
        "f2" => b"\x1bOQ",
        "f3" => b"\x1bOR",
        "f4" => b"\x1bOS",
        "f5" => b"\x1b[15~",
        "f6" => b"\x1b[17~",
        "f7" => b"\x1b[18~",
        "f8" => b"\x1b[19~",
        "f9" => b"\x1b[20~",
        "f10" => b"\x1b[21~",
        "f11" => b"\x1b[23~",
        "f12" => b"\x1b[24~",
        _ => {
            if modifiers.control {
                let letter = key.key.as_bytes();
                if letter.len() == 1 && letter[0].is_ascii_alphabetic() {
                    return Some(vec![letter[0].to_ascii_lowercase() - b'a' + 1]);
                }
                return match key.key.as_str() {
                    "space" | "2" => Some(vec![0]),
                    "[" => Some(vec![27]),
                    "\\" => Some(vec![28]),
                    "]" => Some(vec![29]),
                    _ => None,
                };
            }
            return None; // Printable text is delivered by GPUI's native input handler.
        }
    };
    let mut bytes = sequence.to_vec();
    if modifiers.alt {
        bytes.insert(0, 27);
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(key: &str, control: bool) -> gpui::Keystroke {
        gpui::Keystroke {
            key: key.into(),
            key_char: None,
            modifiers: gpui::Modifiers {
                control,
                ..Default::default()
            },
        }
    }

    #[test]
    fn terminal_keys_send_control_sequences_without_inserting_printable_text_twice() {
        assert_eq!(key_bytes(&stroke("c", true), false), Some(vec![3]));
        assert_eq!(
            key_bytes(&stroke("up", false), true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            key_bytes(&stroke("up", false), false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(key_bytes(&stroke("a", false), false), None);
    }

    #[test]
    fn pty_shell_receives_input_and_parses_output() {
        let root = std::env::temp_dir();
        let terminal = Terminal::new(&root, "test".into(), (24, 80)).unwrap();
        terminal.send(b"printf 'GERE_PTY_OK\\n'\r".to_vec());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if terminal
                .screen
                .lock()
                .unwrap()
                .screen()
                .contents()
                .contains("GERE_PTY_OK\n")
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "PTY did not return shell output"
            );
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }

    #[test]
    fn parser_handles_cursor_motion_and_alternate_screen() {
        let mut parser = vt100::Parser::new(3, 20, 20);
        parser.process(b"hello\rworld\x1b[2;1Hnext");
        assert_eq!(parser.screen().rows(0, 20).collect::<Vec<_>>()[0], "world");
        assert_eq!(parser.screen().rows(0, 20).collect::<Vec<_>>()[1], "next");
        parser.process(b"\x1b[?1049hother");
        assert!(parser.screen().alternate_screen());
        parser.process(b"\x1b[?1049l");
        assert_eq!(parser.screen().rows(0, 20).collect::<Vec<_>>()[0], "world");
    }

    #[test]
    fn parser_keeps_ten_thousand_scrollback_lines() {
        let mut parser = vt100::Parser::new(2, 12, SCROLLBACK_LINES);
        let output = (0..10_100).map(|i| format!("{i}\r\n")).collect::<String>();
        parser.process(output.as_bytes());
        parser.screen_mut().set_scrollback(usize::MAX);
        assert_eq!(parser.screen().scrollback(), SCROLLBACK_LINES);
    }
}
