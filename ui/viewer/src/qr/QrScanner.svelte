<script lang="ts">
  // Scans a QR code with the camera. The frames never leave the page: each one is drawn on a
  // canvas and read here, and the stream stops when the dialog closes.
  import { onMount } from "svelte";

  import { readQrCode, tripPayloadOfQr } from "./qr-reader";

  let {
    onPayload,
    onClose,
  }: {
    /** Receives the payload of the trip in a code that the camera found. */
    onPayload: (payload: string) => void;
    onClose: () => void;
  } = $props();

  type CameraState = "starting" | "scanning" | "denied" | "unavailable" | "failed";

  /** Longest side, in pixels, of the frames that are read. */
  const frameSide = 800;
  /** Time between two reads, in milliseconds. Reading a frame takes a few dozen. */
  const readInterval = 150;

  let video: HTMLVideoElement | undefined = $state();
  let cameraState: CameraState = $state("starting");
  /** Set when the camera sees a code that is no trip, so the traveller knows why nothing opens. */
  let foreignCode = $state(false);

  onMount(() => {
    let stream: MediaStream | null = null;
    let timer: number | undefined;
    let stopped = false;
    const canvas = document.createElement("canvas");

    /** Reads the current frame and hands over the trip it holds, if any. */
    function readFrame() {
      if (video === undefined || video.videoWidth === 0 || stopped) {
        return;
      }
      const scale = Math.min(1, frameSide / Math.max(video.videoWidth, video.videoHeight));
      canvas.width = Math.round(video.videoWidth * scale);
      canvas.height = Math.round(video.videoHeight * scale);
      const context = canvas.getContext("2d", { willReadFrequently: true });
      if (context === null) {
        return;
      }
      context.drawImage(video, 0, 0, canvas.width, canvas.height);
      const text = readQrCode(context.getImageData(0, 0, canvas.width, canvas.height));
      if (text === null) {
        return;
      }
      const payload = tripPayloadOfQr(text);
      if (payload === null) {
        foreignCode = true;
        return;
      }
      stop();
      onPayload(payload);
    }

    function stop() {
      stopped = true;
      window.clearInterval(timer);
      stream?.getTracks().forEach((track) => track.stop());
      stream = null;
    }

    async function start() {
      if (navigator.mediaDevices?.getUserMedia === undefined) {
        cameraState = "unavailable";
        return;
      }
      try {
        stream = await navigator.mediaDevices.getUserMedia({
          video: { facingMode: { ideal: "environment" }, width: { ideal: 1280 } },
          audio: false,
        });
      } catch (error) {
        const name = error instanceof DOMException ? error.name : "";
        cameraState = name === "NotAllowedError" || name === "SecurityError" ? "denied" : name === "NotFoundError" ? "unavailable" : "failed";
        return;
      }
      if (stopped || video === undefined) {
        stream.getTracks().forEach((track) => track.stop());
        return;
      }
      video.srcObject = stream;
      try {
        await video.play();
      } catch {
        cameraState = "failed";
        stop();
        return;
      }
      cameraState = "scanning";
      timer = window.setInterval(readFrame, readInterval);
    }

    void start();
    return stop;
  });
</script>

<div class="backdrop" role="presentation">
  <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="scan-title">
    <h2 id="scan-title">Scan a QR code</h2>
    <div class="viewfinder">
      <!-- svelte-ignore a11y_media_has_caption -->
      <video bind:this={video} playsinline muted></video>
    </div>
    {#if cameraState === "starting"}
      <p class="message">Starting the camera…</p>
    {:else if cameraState === "scanning"}
      <p class="message">
        {foreignCode
          ? "That QR code is not a trip from BigTrainTrip. Keep looking."
          : "Hold the QR code of a trip in front of the camera. Nothing leaves your device."}
      </p>
    {:else if cameraState === "denied"}
      <p class="message">
        The browser did not give this page the camera. Allow the camera for this page in the settings
        of the browser, or load a picture of the QR code instead.
      </p>
    {:else if cameraState === "unavailable"}
      <p class="message">This device has no camera that the page can use. Load a picture of the QR code instead.</p>
    {:else}
      <p class="message">The camera did not start. Load a picture of the QR code instead.</p>
    {/if}
    <div class="actions">
      <button type="button" onclick={onClose}>Close</button>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 10;
    display: grid;
    place-items: center;
    background: rgb(0 0 0 / 45%);
  }

  .dialog {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    width: min(520px, calc(100vw - 2 * var(--space-4)));
    padding: var(--space-4);
    border: 1px solid var(--color-border);
    border-radius: var(--radius);
    background: var(--color-surface);
  }

  h2 {
    margin: 0;
    font-size: var(--text-heading);
  }

  .viewfinder {
    overflow: hidden;
    border-radius: var(--radius);
    background: #000000;
    aspect-ratio: 4 / 3;
  }

  video {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .message {
    margin: 0;
    color: var(--color-text-muted);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
  }
</style>
