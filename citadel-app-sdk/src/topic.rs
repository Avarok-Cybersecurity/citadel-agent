//! `topic`: the app's named channels between the members who installed and allowed it (a
//! Citadel group per install and topic, never a chat group). Open one with [`open`]; its
//! events go to the handler given there, with the app. A topic announces; bulk data goes over
//! internal HTTP.

pub use citadel_app_sdk_types::calls::topic::{
    is_topic_name, LIMIT_TOPIC_BYTES, LIMIT_TOPIC_RATE, MAX_TOPICS,
};
pub use citadel_app_sdk_types::calls::TopicEvent;

use crate::abi::RawAbi;
use crate::pending::{start, Pending};
use citadel_app_sdk_types::calls::{TopicClose, TopicOpen, TopicPublish};
use citadel_app_sdk_types::{Bytes, Refused};
use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeMap;

type Sink = Box<dyn FnMut(&mut dyn Any, TopicEvent)>;

struct Open {
    sink: Option<Sink>,
    members: Vec<String>,
}

thread_local! {
    static TOPICS: RefCell<BTreeMap<String, Open>> = const { RefCell::new(BTreeMap::new()) };
}

/// An open topic. Closing it is explicit ([`Topic::close`]); dropping it keeps it open.
#[derive(Debug, PartialEq, Eq)]
pub struct Topic {
    name: String,
}

pub(crate) fn open_with<A: 'static>(
    raw: &impl RawAbi,
    name: &str,
    mut on: impl FnMut(&mut A, TopicEvent) + 'static,
) -> Pending<Topic> {
    let sink: Sink = Box::new(move |app, event| {
        let app = app
            .downcast_mut::<A>()
            .expect("topic names the app's own type");
        on(app, event)
    });
    let open = Open {
        sink: Some(sink),
        members: Vec::new(),
    };
    TOPICS.with(|t| t.borrow_mut().insert(name.to_string(), open));
    start(raw, &TopicOpen { name: name.into() }).map(|opened| {
        TOPICS.with(|t| {
            if let Some(open) = t.borrow_mut().get_mut(&opened.name) {
                open.members = opened.members;
            }
        });
        Topic { name: opened.name }
    })
}

impl Topic {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Who is on the topic, as its last `Members` event (or the open reply) said.
    pub fn members(&self) -> Vec<String> {
        TOPICS
            .with(|t| t.borrow().get(&self.name).map(|o| o.members.clone()))
            .unwrap_or_default()
    }

    pub(crate) fn publish_with(&self, raw: &impl RawAbi, body: &[u8]) -> Result<(), Refused> {
        let publish = TopicPublish {
            name: self.name.clone(),
            body: Bytes(body.to_vec()),
        };
        let mut sent = start(raw, &publish);
        match sent.take_ready() {
            Some(result) => result,
            None => {
                sent.watch("topic publish");
                Ok(())
            }
        }
    }

    pub(crate) fn close_with(self, raw: &impl RawAbi) {
        TOPICS.with(|t| t.borrow_mut().remove(&self.name));
        start(raw, &TopicClose { name: self.name }).watch("topic close");
    }
}

/// A topic event: the open topic's handler's (`None`), or the app's when no handler has it.
pub(crate) fn route(
    app: &mut dyn Any,
    name: String,
    event: TopicEvent,
) -> Option<(String, TopicEvent)> {
    let sink = TOPICS.with(|t| {
        let mut t = t.borrow_mut();
        let open = t.get_mut(&name)?;
        if let TopicEvent::Members(members) = &event {
            open.members = members.clone();
        }
        open.sink.take()
    });
    let Some(mut sink) = sink else {
        return Some((name, event));
    };
    sink(app, event);
    TOPICS.with(|t| {
        if let Some(open) = t.borrow_mut().get_mut(&name) {
            open.sink.get_or_insert(sink);
        }
    });
    None
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use crate::ffi::WASM;

    /// Join topic `name` of this install; its events go to `on`, with the app.
    pub fn open<A: 'static>(
        name: &str,
        on: impl FnMut(&mut A, TopicEvent) + 'static,
    ) -> Pending<Topic> {
        open_with(&WASM, name, on)
    }

    impl Topic {
        /// One message to everyone on the topic. A rate or size refusal is answered at once.
        pub fn publish(&self, body: &[u8]) -> Result<(), Refused> {
            self.publish_with(&WASM, body)
        }

        /// Leave the topic.
        pub fn close(self) {
            self.close_with(&WASM)
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm::*;

#[cfg(test)]
#[path = "topic_tests.rs"]
mod tests;
