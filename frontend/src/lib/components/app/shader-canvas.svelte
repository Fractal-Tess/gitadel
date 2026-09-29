<!--
  Runs one fragment shader over a full-size quad with raw WebGL. Supplies
  `uRes` (drawing-buffer pixels) and `uTime` (seconds); every other uniform
  comes from `uniforms`, sized by value: a number is a float, a 2- or 3-tuple a
  vec2 or vec3. The loop pauses while the tab is hidden or the canvas is
  off-screen, draws one still frame under reduced motion, and leaves the plain
  background colour showing when WebGL is unavailable or the context is lost.
-->
<script lang="ts" module>
  export type UniformValue = number | readonly number[];

  /** Parses `#rgb` or `#rrggbb` into 0..1 channels; black otherwise. */
  export function hexToRgb(hex: string): [number, number, number] {
    let value = hex.trim().replace(/^#/u, "");
    if (value.length === 3) value = [...value].map((c) => c + c).join("");
    const parsed = Number.parseInt(value, 16);
    if (value.length !== 6 || Number.isNaN(parsed)) return [0, 0, 0];
    return [
      ((parsed >> 16) & 255) / 255,
      ((parsed >> 8) & 255) / 255,
      (parsed & 255) / 255,
    ];
  }
</script>

<script lang="ts">
  import { onMount, type Snippet } from "svelte";

  interface Props {
    /** GLSL ES 1.0 fragment shader; receives `varying vec2 vUv`. */
    fragment: string;
    uniforms: Record<string, UniformValue>;
    backgroundColor: string;
    /** Upper bound on device pixels per CSS pixel. */
    dpr?: number;
    /** Extra drawing-buffer scale below `dpr`; the browser upscales. */
    renderScale?: number;
    /** Seconds added to the clock, so the still frame can pick its moment. */
    timeOffset?: number;
    class?: string;
    children?: Snippet;
  }

  let {
    fragment,
    uniforms,
    backgroundColor,
    dpr = 1.5,
    renderScale = 1,
    timeOffset = 0,
    class: className = "",
    children,
  }: Props = $props();

  const VERTEX_SHADER = `
    attribute vec2 aPosition;
    varying vec2 vUv;
    void main() {
      vUv = aPosition * 0.5 + 0.5;
      gl_Position = vec4(aPosition, 0.0, 1.0);
    }
  `;

  let canvas: HTMLCanvasElement;
  let unavailable = $state(false);
  // Set once WebGL is ready; redraws when the loop is not already running.
  let requestFrame: (() => void) | null = null;

  $effect(() => {
    void uniforms;
    void dpr;
    void renderScale;
    requestFrame?.();
  });

  function compile(
    gl: WebGLRenderingContext,
    type: number,
    source: string,
  ): WebGLShader | null {
    const shader = gl.createShader(type);
    if (!shader) return null;
    gl.shaderSource(shader, source);
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
      console.warn("[shader-canvas]", gl.getShaderInfoLog(shader));
      gl.deleteShader(shader);
      return null;
    }
    return shader;
  }

  onMount(() => {
    const context = canvas.getContext("webgl", {
      antialias: false,
      alpha: true,
      premultipliedAlpha: false,
      powerPreference: "low-power",
    });
    if (!context) {
      unavailable = true;
      return;
    }
    const gl = context;
    // Lets shaders opt into fwidth() with `#extension`; core in WebGL 2.
    gl.getExtension("OES_standard_derivatives");

    const vertex = compile(gl, gl.VERTEX_SHADER, VERTEX_SHADER);
    const fragmentShader = compile(gl, gl.FRAGMENT_SHADER, fragment);
    const program = gl.createProgram();
    const release = () => {
      gl.deleteProgram(program);
      gl.deleteShader(vertex);
      gl.deleteShader(fragmentShader);
      gl.getExtension("WEBGL_lose_context")?.loseContext();
    };
    if (!vertex || !fragmentShader || !program) {
      unavailable = true;
      release();
      return;
    }
    gl.attachShader(program, vertex);
    gl.attachShader(program, fragmentShader);
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      console.warn("[shader-canvas]", gl.getProgramInfoLog(program));
      unavailable = true;
      release();
      return;
    }
    gl.useProgram(program);

    // One oversized triangle covers the viewport.
    const buffer = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
    gl.bufferData(
      gl.ARRAY_BUFFER,
      new Float32Array([-1, -1, 3, -1, -1, 3]),
      gl.STATIC_DRAW,
    );
    const position = gl.getAttribLocation(program, "aPosition");
    gl.enableVertexAttribArray(position);
    gl.vertexAttribPointer(position, 2, gl.FLOAT, false, 0, 0);

    const locations = new Map<string, WebGLUniformLocation | null>();
    const location = (name: string) => {
      if (!locations.has(name)) {
        locations.set(name, gl.getUniformLocation(program, name));
      }
      return locations.get(name) ?? null;
    };

    const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
    const startedAt = performance.now();
    let frame = 0;
    let onScreen = true;
    let lost = false;

    function resize(): void {
      const ratio = Math.min(window.devicePixelRatio || 1, dpr) * renderScale;
      const width = Math.max(1, Math.round(canvas.clientWidth * ratio));
      const height = Math.max(1, Math.round(canvas.clientHeight * ratio));
      if (canvas.width !== width || canvas.height !== height) {
        canvas.width = width;
        canvas.height = height;
      }
    }

    function draw(now: number): void {
      if (lost) return;
      resize();
      gl.viewport(0, 0, canvas.width, canvas.height);
      gl.uniform2f(location("uRes"), canvas.width, canvas.height);
      const elapsed = reducedMotion.matches ? 0 : (now - startedAt) / 1000;
      gl.uniform1f(location("uTime"), elapsed + timeOffset);
      for (const [name, value] of Object.entries(uniforms)) {
        const target = location(name);
        if (typeof value === "number") gl.uniform1f(target, value);
        else if (value.length === 2) gl.uniform2f(target, value[0], value[1]);
        else if (value.length === 3) {
          gl.uniform3f(target, value[0], value[1], value[2]);
        }
      }
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    }

    const animating = () =>
      !lost && onScreen && !document.hidden && !reducedMotion.matches;

    function tick(now: number): void {
      draw(now);
      frame = animating() ? requestAnimationFrame(tick) : 0;
    }

    function sync(): void {
      if (animating()) {
        if (!frame) frame = requestAnimationFrame(tick);
      } else {
        cancelAnimationFrame(frame);
        frame = 0;
        if (!lost && onScreen) draw(performance.now());
      }
    }

    requestFrame = () => {
      if (!frame) draw(performance.now());
    };

    const resizeObserver = new ResizeObserver(() => requestFrame?.());
    resizeObserver.observe(canvas);
    const intersectionObserver = new IntersectionObserver((entries) => {
      onScreen = entries.some((entry) => entry.isIntersecting);
      sync();
    });
    intersectionObserver.observe(canvas);

    function onContextLost(event: Event): void {
      event.preventDefault();
      lost = true;
      unavailable = true;
      sync();
    }

    canvas.addEventListener("webglcontextlost", onContextLost);
    document.addEventListener("visibilitychange", sync);
    reducedMotion.addEventListener("change", sync);
    sync();

    return () => {
      requestFrame = null;
      cancelAnimationFrame(frame);
      resizeObserver.disconnect();
      intersectionObserver.disconnect();
      canvas.removeEventListener("webglcontextlost", onContextLost);
      document.removeEventListener("visibilitychange", sync);
      reducedMotion.removeEventListener("change", sync);
      gl.deleteBuffer(buffer);
      release();
    };
  });
</script>

<div
  class={["relative isolate overflow-hidden", className]}
  style:background-color={backgroundColor}
>
  <canvas
    bind:this={canvas}
    class={["absolute inset-0 -z-10 size-full", unavailable && "hidden"]}
    aria-hidden="true"
  ></canvas>
  {@render children?.()}
</div>
