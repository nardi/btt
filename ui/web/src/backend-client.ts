// The page's side of the connection to the worker that runs the backend.

/** An answer from the worker. */
type WorkerAnswer =
  | { kind: "result"; id: number; json: string }
  | { kind: "failure"; id: number; message: string }
  | { kind: "channel"; channelId: number; json: string }
  | { kind: "file"; name: string; contents: string };

/** A command that waits for its answer. */
interface PendingCommand {
  resolve: (value: unknown) => void;
  reject: (reason: unknown) => void;
}

/** Talks to the worker. */
class BackendClient {
  private readonly worker: Worker;
  private readonly pending = new Map<number, PendingCommand>();
  private nextCommandId = 1;
  private channelListener: (channelId: number, json: string) => void = () => {};

  constructor() {
    this.worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
    this.worker.onmessage = (event: MessageEvent<WorkerAnswer>) => this.receive(event.data);
  }

  /** Sets the function that receives the messages of channels. */
  onChannelMessage(listener: (channelId: number, json: string) => void): void {
    this.channelListener = listener;
  }

  /** Runs a command in the worker. */
  invoke<T>(command: string, args: Record<string, unknown>): Promise<T> {
    const id = this.nextCommandId++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (value: unknown) => void, reject });
      this.worker.postMessage({ id, command, args: JSON.stringify(args) });
    });
  }

  private receive(answer: WorkerAnswer): void {
    switch (answer.kind) {
      case "result": {
        const pending = this.pending.get(answer.id);
        this.pending.delete(answer.id);
        pending?.resolve(answer.json === "" ? null : JSON.parse(answer.json));
        break;
      }
      case "failure": {
        const pending = this.pending.get(answer.id);
        this.pending.delete(answer.id);
        pending?.reject(readFailure(answer.message));
        break;
      }
      case "channel":
        this.channelListener(answer.channelId, answer.json);
        break;
      case "file":
        offerDownload(answer.name, answer.contents);
        break;
    }
  }
}

/**
 * Turns the text of a failure into what the generated bindings expect. An error of the backend
 * is JSON and becomes a value, like the error a Tauri command returns. Any other failure becomes
 * an `Error`, which the bindings rethrow.
 */
function readFailure(message: string): unknown {
  try {
    return JSON.parse(message);
  } catch {
    return new Error(message);
  }
}

/** Offers a text as a download. */
function offerDownload(name: string, contents: string): void {
  const url = URL.createObjectURL(new Blob([contents], { type: "application/json" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  // The browser has started the download by the time the next task runs.
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

export const backend = new BackendClient();
