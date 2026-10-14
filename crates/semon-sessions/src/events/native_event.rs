//! The native producer's exact event shape with descriptor-capable text.
//! Complete parsing uses String; bounded Original repair uses qualified native
//! field references. No nested reply/peer/yield may hide an owned giant string.

use super::{Event, Kind, Peer, Reply, Yield};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NativeReply<T> {
    pub(crate) o: u64,
    pub(crate) b: u32,
    pub(crate) t: Option<i64>,
    pub(crate) e: bool,
    pub(crate) f: u8,
    pub(crate) m: Option<T>,
}
impl<T> Default for NativeReply<T> {
    fn default() -> Self {
        Self {
            o: 0,
            b: 0,
            t: None,
            e: false,
            f: 0,
            m: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NativePeer<T> {
    pub(crate) sock: Option<T>,
    pub(crate) pid: Option<u64>,
    pub(crate) start: Option<T>,
}
impl<T> Default for NativePeer<T> {
    fn default() -> Self {
        Self {
            sock: None,
            pid: None,
            start: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NativeYield<T> {
    pub(crate) polls: Vec<u64>,
    pub(crate) cut: bool,
    pub(crate) done: Option<NativeReply<T>>,
}
impl<T> Default for NativeYield<T> {
    fn default() -> Self {
        Self {
            polls: Vec::new(),
            cut: false,
            done: None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NativeEvent<T> {
    pub(crate) k: Kind,
    pub(crate) o: u64,
    pub(crate) b: u32,
    pub(crate) t: Option<i64>,
    pub(crate) id: Option<T>,
    pub(crate) n: Option<T>,
    pub(crate) code_mode: bool,
    pub(crate) code_mode_open: bool,
    pub(crate) parent: Option<usize>,
    pub(crate) script: u8,
    pub(crate) y: Option<Box<NativeYield<T>>>,
    pub(crate) poll: Option<usize>,
    pub(crate) r: Option<NativeReply<T>>,
    pub(crate) item: Option<u64>,
    pub(crate) peer: Option<NativePeer<T>>,
    pub(crate) u: Option<T>,
}
impl<T> Default for NativeEvent<T> {
    fn default() -> Self {
        Self {
            k: Kind::default(),
            o: 0,
            b: 0,
            t: None,
            id: None,
            n: None,
            code_mode: false,
            code_mode_open: false,
            parent: None,
            script: 0,
            y: None,
            poll: None,
            r: None,
            item: None,
            peer: None,
            u: None,
        }
    }
}
impl<T> NativeReply<T> {
    fn scalar(&self) -> Reply {
        Reply {
            o: self.o,
            b: self.b,
            t: self.t,
            e: self.e,
            f: self.f,
            m: None,
        }
    }
}
impl<T> NativeEvent<T> {
    /// Candidate/position/time reducers use the native scalar fields. Text
    /// identity, native copy equality and relation placement remain explicit
    /// descriptor operations at their existing owners; absence here is never
    /// evidence that a native id/name/UUID/socket did not exist.
    pub(crate) fn scalar(&self) -> Event {
        Event {
            k: self.k,
            o: self.o,
            b: self.b,
            t: self.t,
            id: None,
            n: None,
            code_mode: self.code_mode,
            code_mode_open: self.code_mode_open,
            parent: self.parent,
            script: self.script,
            y: self.y.as_ref().map(|yielded| {
                Box::new(Yield {
                    polls: yielded.polls.clone(),
                    cut: yielded.cut,
                    done: yielded.done.as_ref().map(NativeReply::scalar),
                })
            }),
            poll: self.poll,
            r: self.r.as_ref().map(NativeReply::scalar),
            item: self.item,
            peer: self.peer.as_ref().map(|peer| Peer {
                sock: None,
                pid: peer.pid,
                start: None,
            }),
            u: None,
        }
    }
}
impl From<Reply> for NativeReply<String> {
    fn from(value: Reply) -> Self {
        Self {
            o: value.o,
            b: value.b,
            t: value.t,
            e: value.e,
            f: value.f,
            m: value.m,
        }
    }
}
impl From<NativeReply<String>> for Reply {
    fn from(value: NativeReply<String>) -> Self {
        Self {
            o: value.o,
            b: value.b,
            t: value.t,
            e: value.e,
            f: value.f,
            m: value.m,
        }
    }
}
impl From<Event> for NativeEvent<String> {
    fn from(value: Event) -> Self {
        Self {
            k: value.k,
            o: value.o,
            b: value.b,
            t: value.t,
            id: value.id,
            n: value.n,
            code_mode: value.code_mode,
            code_mode_open: value.code_mode_open,
            parent: value.parent,
            script: value.script,
            y: value.y.map(|yielded| {
                Box::new(NativeYield {
                    polls: yielded.polls,
                    cut: yielded.cut,
                    done: yielded.done.map(Into::into),
                })
            }),
            poll: value.poll,
            r: value.r.map(Into::into),
            item: value.item,
            peer: value.peer.map(|peer| NativePeer {
                sock: peer.sock,
                pid: peer.pid,
                start: peer.start,
            }),
            u: value.u,
        }
    }
}
impl From<NativeEvent<String>> for Event {
    fn from(value: NativeEvent<String>) -> Self {
        Self {
            k: value.k,
            o: value.o,
            b: value.b,
            t: value.t,
            id: value.id,
            n: value.n,
            code_mode: value.code_mode,
            code_mode_open: value.code_mode_open,
            parent: value.parent,
            script: value.script,
            y: value.y.map(|yielded| {
                Box::new(Yield {
                    polls: yielded.polls,
                    cut: yielded.cut,
                    done: yielded.done.map(Into::into),
                })
            }),
            poll: value.poll,
            r: value.r.map(Into::into),
            item: value.item,
            peer: value.peer.map(|peer| Peer {
                sock: peer.sock,
                pid: peer.pid,
                start: peer.start,
            }),
            u: value.u,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_event_roundtrip_preserves_all_nested_native_fields() {
        let event = Event {
            k: Kind::Tool,
            o: 10,
            b: 2,
            t: Some(3),
            id: Some("id".into()),
            n: Some("name".into()),
            code_mode: true,
            code_mode_open: true,
            parent: Some(9),
            script: 7,
            y: Some(Box::new(Yield {
                polls: vec![20, 30],
                cut: true,
                done: Some(Reply {
                    o: 40,
                    b: 5,
                    t: Some(6),
                    e: true,
                    f: 2,
                    m: Some("done-message".into()),
                }),
            })),
            poll: Some(8),
            r: Some(Reply {
                o: 50,
                b: 3,
                t: Some(7),
                e: true,
                f: 4,
                m: Some("message".into()),
            }),
            item: Some(60),
            peer: Some(Peer {
                sock: Some("socket".into()),
                pid: Some(70),
                start: Some("start".into()),
            }),
            u: Some("uuid".into()),
        };
        let native: NativeEvent<String> = event.clone().into();
        assert_eq!(Event::from(native.clone()), event);
        let scalar = native.scalar();
        assert_eq!(
            (
                scalar.k,
                scalar.o,
                scalar.b,
                scalar.t,
                scalar.parent,
                scalar.script,
                scalar.poll,
                scalar.item
            ),
            (
                event.k,
                event.o,
                event.b,
                event.t,
                event.parent,
                event.script,
                event.poll,
                event.item
            )
        );
        assert!(scalar.id.is_none() && scalar.n.is_none() && scalar.u.is_none());
        assert!(scalar.r.unwrap().m.is_none());
        assert!(scalar.y.unwrap().done.unwrap().m.is_none());
        let peer = scalar.peer.unwrap();
        assert_eq!(peer.pid, Some(70));
        assert!(peer.sock.is_none() && peer.start.is_none());
    }
}
