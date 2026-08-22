<script lang="ts">
  import { onDestroy } from "svelte";
  import { ImagePlus, ZoomIn, ZoomOut } from "lucide-svelte";
  import type { Attachment } from "svelte/attachments";

  import {
    clampCropOffset,
    coverScale,
    renderAvatarPng,
  } from "$lib/avatar-crop.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import { Slider } from "$lib/components/ui/slider/index.js";

  const MAX_SOURCE_BYTES = 10 * 1024 * 1024;
  const MAX_SOURCE_PIXELS = 40_000_000;
  const ACCEPTED_TYPES = new Set(["image/jpeg", "image/png", "image/webp"]);

  let {
    open = $bindable(false),
    onsave,
  }: {
    open?: boolean;
    onsave: (blob: Blob) => Promise<void>;
  } = $props();

  let fileInput: HTMLInputElement;
  let image = $state.raw<HTMLImageElement | null>(null);
  let sourceUrl = $state("");
  let cropSize = $state(256);
  let zoom = $state(1);
  let offsetX = $state(0);
  let offsetY = $state(0);
  let decoding = $state(false);
  let saving = $state(false);
  let error = $state<string | null>(null);
  let decodeSequence = 0;
  let activePointer: number | null = null;
  let dragStartX = 0;
  let dragStartY = 0;
  let dragOffsetX = 0;
  let dragOffsetY = 0;

  const imageStyle = $derived.by(() => {
    if (!image) return "";
    const scale =
      coverScale(image.naturalWidth, image.naturalHeight, cropSize) * zoom;
    return `width:${image.naturalWidth * scale}px;height:${image.naturalHeight * scale}px;transform:translate(calc(-50% + ${offsetX}px),calc(-50% + ${offsetY}px))`;
  });

  const observeCropStage: Attachment<HTMLElement> = (element) => {
    const updateSize = () => {
      const nextSize = Math.max(1, element.clientWidth - 32);
      const ratio = nextSize / cropSize;
      offsetX *= ratio;
      offsetY *= ratio;
      cropSize = nextSize;
      clampOffsets();
    };
    const observer = new ResizeObserver(updateSize);
    observer.observe(element);
    updateSize();
    return () => observer.disconnect();
  };

  onDestroy(resetEditor);

  function setOpen(nextOpen: boolean) {
    if (!nextOpen && saving) return;
    open = nextOpen;
    if (!nextOpen) resetEditor();
  }

  function resetEditor() {
    decodeSequence += 1;
    if (sourceUrl) URL.revokeObjectURL(sourceUrl);
    sourceUrl = "";
    image = null;
    zoom = 1;
    offsetX = 0;
    offsetY = 0;
    decoding = false;
    error = null;
    activePointer = null;
    if (fileInput) fileInput.value = "";
  }

  async function selectFile(file: File | undefined) {
    if (!file) return;
    error = null;
    if (!ACCEPTED_TYPES.has(file.type)) {
      error = "Choose a JPG, PNG, or WebP image.";
      return;
    }
    if (file.size > MAX_SOURCE_BYTES) {
      error = "Choose an image smaller than 10 MB.";
      return;
    }

    decoding = true;
    const sequence = ++decodeSequence;
    const nextUrl = URL.createObjectURL(file);
    let adopted = false;
    try {
      const nextImage = new Image();
      nextImage.decoding = "async";
      nextImage.src = nextUrl;
      await nextImage.decode();
      if (sequence !== decodeSequence) return;
      if (
        nextImage.naturalWidth * nextImage.naturalHeight >
        MAX_SOURCE_PIXELS
      ) {
        throw new Error("Choose an image smaller than 40 megapixels.");
      }
      if (sourceUrl) URL.revokeObjectURL(sourceUrl);
      sourceUrl = nextUrl;
      adopted = true;
      image = nextImage;
      zoom = 1;
      offsetX = 0;
      offsetY = 0;
    } catch (caught) {
      if (sequence === decodeSequence) {
        error =
          caught instanceof Error
            ? caught.message
            : "This image could not be opened.";
      }
    } finally {
      if (!adopted) URL.revokeObjectURL(nextUrl);
      if (sequence === decodeSequence) decoding = false;
      if (fileInput) fileInput.value = "";
    }
  }

  function clampOffsets() {
    if (!image) return;
    const scale =
      coverScale(image.naturalWidth, image.naturalHeight, cropSize) * zoom;
    offsetX = clampCropOffset(offsetX, image.naturalWidth, scale, cropSize);
    offsetY = clampCropOffset(offsetY, image.naturalHeight, scale, cropSize);
  }

  function setZoom(nextZoom: number) {
    if (!image) return;
    const ratio = nextZoom / zoom;
    offsetX *= ratio;
    offsetY *= ratio;
    zoom = nextZoom;
    clampOffsets();
  }

  function startDrag(event: PointerEvent) {
    if (!image || !(event.currentTarget instanceof HTMLElement)) return;
    activePointer = event.pointerId;
    dragStartX = event.clientX;
    dragStartY = event.clientY;
    dragOffsetX = offsetX;
    dragOffsetY = offsetY;
    event.currentTarget.setPointerCapture(event.pointerId);
  }

  function drag(event: PointerEvent) {
    if (event.pointerId !== activePointer) return;
    offsetX = dragOffsetX + event.clientX - dragStartX;
    offsetY = dragOffsetY + event.clientY - dragStartY;
    clampOffsets();
  }

  function stopDrag(event: PointerEvent) {
    if (
      event.pointerId !== activePointer ||
      !(event.currentTarget instanceof HTMLElement)
    )
      return;
    activePointer = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  }

  function handleCropKeydown(event: KeyboardEvent) {
    if (!image) return;
    const step = event.shiftKey ? 16 : 4;
    if (event.key === "ArrowLeft") offsetX -= step;
    else if (event.key === "ArrowRight") offsetX += step;
    else if (event.key === "ArrowUp") offsetY -= step;
    else if (event.key === "ArrowDown") offsetY += step;
    else if (event.key === "+" || event.key === "=") {
      setZoom(Math.min(3, zoom + 0.1));
    } else if (event.key === "-") {
      setZoom(Math.max(1, zoom - 0.1));
    } else if (event.key === "Home") {
      zoom = 1;
      offsetX = 0;
      offsetY = 0;
    } else return;
    event.preventDefault();
    clampOffsets();
  }

  function handleDrop(event: DragEvent) {
    event.preventDefault();
    void selectFile(event.dataTransfer?.files[0]);
  }

  async function save() {
    if (!image) return;
    saving = true;
    error = null;
    try {
      const blob = await renderAvatarPng(
        image,
        cropSize,
        zoom,
        offsetX,
        offsetY,
      );
      await onsave(blob);
      saving = false;
      setOpen(false);
    } catch (caught) {
      error =
        caught instanceof Error
          ? caught.message
          : "The profile picture could not be saved.";
      saving = false;
    }
  }
</script>

<Dialog.Root {open} onOpenChange={setOpen}>
  <Dialog.Content
    class="max-h-[calc(100svh-2rem)] overflow-y-auto ring-foreground/20 sm:max-w-lg"
  >
    <Dialog.Header>
      <Dialog.Title>Change profile picture</Dialog.Title>
      <Dialog.Description>
        Choose an image, drag to reposition it, then zoom until the circular
        preview looks right.
      </Dialog.Description>
    </Dialog.Header>

    <input
      bind:this={fileInput}
      class="sr-only"
      type="file"
      accept="image/jpeg,image/png,image/webp"
      aria-label="Choose profile picture"
      onchange={(event) => void selectFile(event.currentTarget.files?.[0])}
    />

    {#if image}
      <div class="grid gap-4">
        <button
          type="button"
          class="relative mx-auto aspect-square w-full max-w-80 touch-none overflow-hidden rounded-lg bg-black p-0 select-none"
          aria-label="Profile picture crop. Drag or use the arrow keys to reposition."
          {@attach observeCropStage}
          onpointerdown={startDrag}
          onpointermove={drag}
          onpointerup={stopDrag}
          onpointercancel={stopDrag}
          onkeydown={handleCropKeydown}
        >
          <img
            class="pointer-events-none absolute top-1/2 left-1/2 max-w-none select-none"
            src={sourceUrl}
            alt=""
            draggable="false"
            style={imageStyle}
          />
          <span
            class="pointer-events-none absolute top-4 right-4 bottom-4 left-4 rounded-full ring-2 ring-white/75"
            style="box-shadow: 0 0 0 999px rgb(0 0 0 / 0.62)"
          ></span>
        </button>

        <div
          class="grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3"
        >
          <ZoomOut class="size-4 text-muted-foreground" aria-hidden="true" />
          <Slider
            type="single"
            value={zoom}
            onValueChange={setZoom}
            min={1}
            max={3}
            step={0.01}
            aria-label="Zoom"
          />
          <ZoomIn class="size-4 text-muted-foreground" aria-hidden="true" />
        </div>

        <div class="flex items-center justify-between gap-3">
          <p class="text-xs text-muted-foreground">
            Arrow keys move · Shift moves faster · +/− zooms
          </p>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onclick={() => fileInput.click()}
          >
            Choose another
          </Button>
        </div>
      </div>
    {:else}
      <button
        type="button"
        class="grid min-h-64 place-items-center rounded-xl border border-dashed bg-background/35 p-8 text-center hover:border-foreground/30 hover:bg-accent/30 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        ondragover={(event) => event.preventDefault()}
        ondrop={handleDrop}
        onclick={() => fileInput.click()}
      >
        <span class="grid justify-items-center gap-3">
          <span
            class="grid size-12 place-items-center rounded-full bg-muted text-muted-foreground"
          >
            <ImagePlus class="size-5" />
          </span>
          <span>
            <span class="block font-medium">Choose an image</span>
            <span class="mt-1 block text-xs text-muted-foreground">
              JPG, PNG, or WebP · up to 10 MB
            </span>
          </span>
        </span>
      </button>
    {/if}

    {#if decoding}
      <p class="text-sm text-muted-foreground" aria-live="polite">
        Opening image…
      </p>
    {/if}
    {#if error}
      <p class="text-sm text-destructive" role="alert">{error}</p>
    {/if}

    <Dialog.Footer>
      <Button
        type="button"
        variant="outline"
        disabled={saving}
        onclick={() => setOpen(false)}
      >
        Cancel
      </Button>
      <Button
        type="button"
        disabled={!image || decoding || saving}
        onclick={() => void save()}
      >
        {saving ? "Saving…" : "Save picture"}
      </Button>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
