export const AVATAR_OUTPUT_SIZE = 512;

export function coverScale(
  imageWidth: number,
  imageHeight: number,
  cropSize: number,
) {
  return Math.max(cropSize / imageWidth, cropSize / imageHeight);
}

export function clampCropOffset(
  offset: number,
  imageSize: number,
  scale: number,
  cropSize: number,
) {
  const limit = Math.max(0, (imageSize * scale - cropSize) / 2);
  return Math.min(limit, Math.max(-limit, offset));
}

export function cropSourceRect(
  imageWidth: number,
  imageHeight: number,
  cropSize: number,
  zoom: number,
  offsetX: number,
  offsetY: number,
) {
  const scale = coverScale(imageWidth, imageHeight, cropSize) * zoom;
  const size = cropSize / scale;
  const centerX = imageWidth / 2 - offsetX / scale;
  const centerY = imageHeight / 2 - offsetY / scale;

  return {
    x: centerX - size / 2,
    y: centerY - size / 2,
    size,
  };
}

export async function blobToBase64(blob: Blob) {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 8192) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
  }
  return btoa(binary);
}

export async function renderAvatarPng(
  image: HTMLImageElement,
  cropSize: number,
  zoom: number,
  offsetX: number,
  offsetY: number,
) {
  const canvas = document.createElement("canvas");
  canvas.width = AVATAR_OUTPUT_SIZE;
  canvas.height = AVATAR_OUTPUT_SIZE;
  const context = canvas.getContext("2d");
  if (!context) throw new Error("This browser cannot prepare the image.");

  const source = cropSourceRect(
    image.naturalWidth,
    image.naturalHeight,
    cropSize,
    zoom,
    offsetX,
    offsetY,
  );
  context.imageSmoothingEnabled = true;
  context.imageSmoothingQuality = "high";
  context.drawImage(
    image,
    source.x,
    source.y,
    source.size,
    source.size,
    0,
    0,
    AVATAR_OUTPUT_SIZE,
    AVATAR_OUTPUT_SIZE,
  );

  const blob = await new Promise<Blob | null>((resolve) =>
    canvas.toBlob(resolve, "image/png"),
  );
  if (!blob) throw new Error("This browser could not prepare the image.");
  return blob;
}
