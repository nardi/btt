//! What the page provides to the module: messages, saved state, saved files and saved legs.

use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures_util::StreamExt;
use wasm_bindgen::JsValue;

/// Something the module hands to the page through the queue.
pub(crate) enum Outgoing {
    /// A message for a channel of the page.
    Channel {
        /// Which channel the page created for the command.
        channel_id: u32,
        /// The message as JSON.
        json: String,
    },
    /// A planned leg, to keep in the browser's storage.
    Leg {
        /// The timetable namespace the leg was planned with.
        namespace: String,
        /// The cache key of the leg.
        key: String,
        /// The planned leg as JSON.
        json: String,
    },
}

/// The functions of the page, and the queue that carries messages to it.
pub(crate) struct Host {
    /// Writes the stored state.
    save_state: js_sys::Function,
    /// Hands an exported file to the page.
    save_file: js_sys::Function,
    /// Sends the outgoing messages. Cheap to clone for each planning session.
    outbox: UnboundedSender<Outgoing>,
}

impl Host {
    /// Wraps the functions of the page and starts delivering queued messages.
    ///
    /// The planner's events come from tasks that must be `Send`, which a JavaScript function is
    /// not. They go through a queue instead, and a task on the event loop delivers them.
    pub(crate) fn new(
        send_message: js_sys::Function,
        save_state: js_sys::Function,
        save_file: js_sys::Function,
        save_leg: js_sys::Function,
    ) -> Self {
        let (outbox, inbox) = unbounded();
        wasm_bindgen_futures::spawn_local(deliver_messages(inbox, send_message, save_leg));
        Self {
            save_state,
            save_file,
            outbox,
        }
    }

    /// Returns a handle that queues messages for the page.
    pub(crate) fn outbox(&self) -> UnboundedSender<Outgoing> {
        self.outbox.clone()
    }

    /// Gives the page the new stored state.
    pub(crate) fn save_state(&self, json: &str) {
        if let Err(error) = self
            .save_state
            .call1(&JsValue::NULL, &JsValue::from_str(json))
        {
            tracing::warn!(?error, "the page could not save the state");
        }
    }

    /// Gives the page a file to offer for download.
    pub(crate) fn save_file(&self, name: &str, contents: &str) {
        let arguments = js_sys::Array::of2(&JsValue::from_str(name), &JsValue::from_str(contents));
        if let Err(error) = self.save_file.apply(&JsValue::NULL, &arguments) {
            tracing::warn!(?error, "the page could not save the file");
        }
    }
}

/// Passes queued messages to the page, in order.
async fn deliver_messages(
    mut inbox: UnboundedReceiver<Outgoing>,
    send: js_sys::Function,
    save_leg: js_sys::Function,
) {
    while let Some(outgoing) = inbox.next().await {
        match outgoing {
            Outgoing::Channel { channel_id, json } => {
                let arguments = js_sys::Array::of2(
                    &JsValue::from_f64(f64::from(channel_id)),
                    &JsValue::from_str(&json),
                );
                if let Err(error) = send.apply(&JsValue::NULL, &arguments) {
                    tracing::warn!(?error, "the page did not take a message");
                }
            }
            Outgoing::Leg {
                namespace,
                key,
                json,
            } => {
                let arguments = js_sys::Array::of3(
                    &JsValue::from_str(&namespace),
                    &JsValue::from_str(&key),
                    &JsValue::from_str(&json),
                );
                if let Err(error) = save_leg.apply(&JsValue::NULL, &arguments) {
                    tracing::warn!(?error, "the page could not save a planned leg");
                }
            }
        }
    }
}
