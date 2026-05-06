use crate::probes::proctrace::VariableEvent as ProcEvent;
use crate::probes::sslsniff::SslEvent;
use crate::probes::procmon::Event as ProcMonEvent;
use crate::probes::filewatch::FileWatchEvent;
use crate::probes::filewrite::FileWriteEvent;

/// Unified event type that can represent any probe event
///
/// Both variants are lightweight and can be stored directly.
#[derive(Debug)]
pub enum Event {
    Ssl(SslEvent),
    Proc(ProcEvent),
    ProcMon(ProcMonEvent),
    FileWatch(FileWatchEvent),
    FileWrite(FileWriteEvent),
}

impl Event {
    /// Get the message type name for logging/debugging
    pub fn event_type(&self) -> &'static str {
        match self {
            Event::Ssl(_) => "Ssl",
            Event::Proc(_) => "Proc",
            Event::ProcMon(_) => "ProcMon",
            Event::FileWatch(_) => "FileWatch",
            Event::FileWrite(_) => "FileWrite",
        }
    }
}

impl Event {
    /// Check if this is an SSL event
    pub fn is_ssl(&self) -> bool {
        matches!(self, Event::Ssl(_))
    }

    /// Check if this is a process event
    pub fn is_proc(&self) -> bool {
        matches!(self, Event::Proc(_))
    }

    /// Check if this is a procmon event
    pub fn is_procmon(&self) -> bool {
        matches!(self, Event::ProcMon(_))
    }

    /// Check if this is a file watch event
    pub fn is_filewatch(&self) -> bool {
        matches!(self, Event::FileWatch(_))
    }

    /// Check if this is a file write event
    pub fn is_filewrite(&self) -> bool {
        matches!(self, Event::FileWrite(_))
    }

    /// Get SSL event if this is one
    pub fn as_ssl(&self) -> Option<&SslEvent> {
        match self {
            Event::Ssl(e) => Some(e),
            _ => None,
        }
    }

    /// Get process event if this is one
    pub fn as_proc(&self) -> Option<&ProcEvent> {
        match self {
            Event::Proc(e) => Some(e),
            _ => None,
        }
    }

    /// Get procmon event if this is one
    pub fn as_procmon(&self) -> Option<&ProcMonEvent> {
        match self {
            Event::ProcMon(e) => Some(e),
            _ => None,
        }
    }

    /// Get file watch event if this is one
    pub fn as_filewatch(&self) -> Option<&FileWatchEvent> {
        match self {
            Event::FileWatch(e) => Some(e),
            _ => None,
        }
    }

    /// Get file write event if this is one
    pub fn as_filewrite(&self) -> Option<&FileWriteEvent> {
        match self {
            Event::FileWrite(e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ssl_event() -> SslEvent {
        SslEvent {
            source: 0,
            timestamp_ns: 100,
            delta_ns: 0,
            pid: 1234,
            tid: 1234,
            uid: 1000,
            len: 5,
            rw: 0,
            comm: "test".to_string(),
            buf: b"hello".to_vec(),
            is_handshake: false,
            ssl_ptr: 0x1000,
        }
    }

    fn make_filewrite_event() -> FileWriteEvent {
        FileWriteEvent {
            pid: 5678,
            tid: 5678,
            uid: 1000,
            timestamp_ns: 200,
            write_size: 10,
            comm: "writer".to_string(),
            filename: "test.jsonl".to_string(),
            buf: b"content".to_vec(),
        }
    }

    fn make_filewatch_event() -> FileWatchEvent {
        FileWatchEvent {
            pid: 9999,
            tid: 9999,
            uid: 0,
            timestamp_ns: 300,
            flags: 0,
            comm: "watcher".to_string(),
            filename: "data.jsonl".to_string(),
        }
    }

    #[test]
    fn test_event_type_ssl() {
        let e = Event::Ssl(make_ssl_event());
        assert_eq!(e.event_type(), "Ssl");
    }

    #[test]
    fn test_event_type_filewrite() {
        let e = Event::FileWrite(make_filewrite_event());
        assert_eq!(e.event_type(), "FileWrite");
    }

    #[test]
    fn test_event_type_filewatch() {
        let e = Event::FileWatch(make_filewatch_event());
        assert_eq!(e.event_type(), "FileWatch");
    }

    #[test]
    fn test_is_ssl() {
        let e = Event::Ssl(make_ssl_event());
        assert!(e.is_ssl());
        assert!(!e.is_proc());
        assert!(!e.is_procmon());
        assert!(!e.is_filewatch());
        assert!(!e.is_filewrite());
    }

    #[test]
    fn test_is_filewrite() {
        let e = Event::FileWrite(make_filewrite_event());
        assert!(e.is_filewrite());
        assert!(!e.is_ssl());
    }

    #[test]
    fn test_is_filewatch() {
        let e = Event::FileWatch(make_filewatch_event());
        assert!(e.is_filewatch());
        assert!(!e.is_ssl());
    }

    #[test]
    fn test_as_ssl_some() {
        let e = Event::Ssl(make_ssl_event());
        let ssl = e.as_ssl().unwrap();
        assert_eq!(ssl.pid, 1234);
    }

    #[test]
    fn test_as_ssl_none() {
        let e = Event::FileWrite(make_filewrite_event());
        assert!(e.as_ssl().is_none());
    }

    #[test]
    fn test_as_filewrite_some() {
        let e = Event::FileWrite(make_filewrite_event());
        let fw = e.as_filewrite().unwrap();
        assert_eq!(fw.pid, 5678);
    }

    #[test]
    fn test_as_filewatch_some() {
        let e = Event::FileWatch(make_filewatch_event());
        let fw = e.as_filewatch().unwrap();
        assert_eq!(fw.pid, 9999);
    }

    #[test]
    fn test_as_proc_none_for_ssl() {
        let e = Event::Ssl(make_ssl_event());
        assert!(e.as_proc().is_none());
        assert!(e.as_procmon().is_none());
        assert!(e.as_filewatch().is_none());
        assert!(e.as_filewrite().is_none());
    }
}
