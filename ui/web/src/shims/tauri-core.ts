// Stands in for `@tauri-apps/api/core`, so the generated bindings call the worker that runs the
// backend instead of a Tauri window.

import { backend } from "../backend-client";

/** Prefix of the text that stands for a channel in the arguments of a command. */
const CHANNEL_PREFIX = "__CHANNEL__:";

/** The channels that wait for messages, by number. */
const channels = new Map<number, Channel<unknown>>();

let nextChannelNumber = 1;

/** Receives the messages that a command sends while it runs, such as planning events. */
export class Channel<T = unknown> {
  readonly id = nextChannelNumber++;
  onmessage: (message: T) => void = () => {};

  constructor() {
    channels.set(this.id, this as Channel<unknown>);
  }

  /** Writes the channel as the text that the backend recognises. */
  toJSON(): string {
    return `${CHANNEL_PREFIX}${this.id}`;
  }
}

/** Runs a command in the backend and returns its result. */
export function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return backend.invoke<T>(command, args ?? {});
}

backend.onChannelMessage((channelId, json) => {
  channels.get(channelId)?.onmessage(JSON.parse(json));
});
