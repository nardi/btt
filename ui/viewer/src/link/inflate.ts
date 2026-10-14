// Inflates raw deflate data, which every browser the viewer targets reads natively.

import { SnapshotLinkError, malformed } from "./errors";

/**
 * Inflates raw deflate data and stops reading when the output passes `limit` bytes, so a small
 * link cannot fill the memory of the tab.
 */
export async function inflateRaw(data: Uint8Array, limit: number): Promise<Uint8Array> {
  const source = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(data);
      controller.close();
    },
  });
  const inflater = new DecompressionStream("deflate-raw") as unknown as ReadableWritablePair<
    Uint8Array,
    Uint8Array
  >;
  const reader = source.pipeThrough(inflater).getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) {
        break;
      }
      total += value.length;
      if (total > limit) {
        await reader.cancel();
        throw new SnapshotLinkError("too-large", "the link inflates to more than the viewer reads");
      }
      chunks.push(value);
    }
  } catch (error) {
    if (error instanceof SnapshotLinkError) {
      throw error;
    }
    throw malformed("the data is not compressed");
  }
  const output = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    output.set(chunk, offset);
    offset += chunk.length;
  }
  return output;
}
