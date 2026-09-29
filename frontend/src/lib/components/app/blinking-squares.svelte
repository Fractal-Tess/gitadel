<!--
  A field of square cells that twinkle, densest toward one edge.
-->
<script lang="ts">
  import type { Snippet } from "svelte";

  import ShaderCanvas, {
    hexToRgb,
  } from "$lib/components/app/shader-canvas.svelte";

  type Direction = "left" | "right" | "top" | "bottom";

  interface Props {
    direction?: Direction;
    gridSize?: number;
    squareColor?: string;
    backgroundColor?: string;
    falloff?: number;
    fadeStart?: number;
    fadeEnd?: number;
    squareSize?: number;
    minBrightness?: number;
    twinkleSpeed?: number;
    twinkleStrength?: number;
    intensity?: number;
    opacity?: number;
    dpr?: number;
    class?: string;
    children?: Snippet;
  }

  let {
    direction = "right",
    gridSize = 52,
    squareColor = "#BB29FF",
    backgroundColor = "#000000",
    falloff = 1.25,
    fadeStart = 0.65,
    fadeEnd = 1,
    squareSize = 0.57,
    minBrightness = 0.55,
    twinkleSpeed = 1.4,
    twinkleStrength = 0.94,
    intensity = 1,
    opacity = 1,
    dpr = 1.5,
    class: className = "",
    children,
  }: Props = $props();

  const DIRECTIONS: Record<Direction, [number, number]> = {
    right: [1, 0],
    left: [-1, 0],
    top: [0, 1],
    bottom: [0, -1],
  };

  const FRAGMENT_SHADER = `
    precision highp float;
    varying vec2 vUv;
    uniform vec2 uRes;
    uniform float uTime;
    uniform float uGrid;
    uniform vec2 uDir;
    uniform float uFalloff;
    uniform float uFadeStart;
    uniform float uFadeEnd;
    uniform float uSquareSize;
    uniform float uMinBright;
    uniform float uTwinkleSpeed;
    uniform float uTwinkleStrength;
    uniform float uIntensity;
    uniform float uAlpha;
    uniform vec3 uSquare;
    uniform vec3 uBg;

    float hash21(vec2 p) {
      p = fract(p * vec2(123.34, 456.21));
      p += dot(p, p + 45.32);
      return fract(p.x * p.y);
    }

    void main() {
      float aspect = uRes.x / max(uRes.y, 1.0);
      vec2 cellsXY = vec2(uGrid * aspect, uGrid);
      if (aspect < 1.0) cellsXY = vec2(uGrid, uGrid / max(aspect, 1e-4));

      vec2 gridUv = vUv * cellsXY;
      vec2 cellId = floor(gridUv);
      vec2 cellUv = fract(gridUv) - 0.5;

      vec2 cellCenter = (cellId + 0.5) / cellsXY;
      vec2 centered = cellCenter * 2.0 - 1.0;
      float t = clamp(dot(centered, uDir) * 0.5 + 0.5, 0.0, 1.0);

      float fs = clamp(uFadeStart, 0.0, 0.999);
      float fe = clamp(uFadeEnd, fs + 0.001, 1.0);
      float density = pow(clamp((t - fs) / (fe - fs), 0.0, 1.0), max(uFalloff, 1e-4));

      float gate = hash21(cellId + 11.7);
      float bRnd = hash21(cellId + 47.3);
      float pRnd = hash21(cellId + 91.1);
      float lit = step(gate, density);

      float half_ = clamp(uSquareSize, 0.05, 0.98) * 0.5;
      float inside = step(abs(cellUv.x), half_) * step(abs(cellUv.y), half_);

      float baseBright = mix(clamp(uMinBright, 0.0, 1.0), 1.0, bRnd);
      float phase = pRnd * 6.2831853;
      float speed = uTwinkleSpeed * (0.6 + 0.8 * bRnd);
      float pulse = 0.5 + 0.5 * sin(uTime * speed + phase);
      float twinkle = mix(1.0 - uTwinkleStrength, 1.0, pulse);

      float mask = inside * lit * baseBright * twinkle * uIntensity;
      gl_FragColor = vec4(mix(uBg, uSquare, clamp(mask, 0.0, 1.0)), uAlpha);
    }
  `;

  const uniforms = $derived({
    uGrid: gridSize,
    uDir: DIRECTIONS[direction] ?? DIRECTIONS.right,
    uFalloff: falloff,
    uFadeStart: fadeStart,
    uFadeEnd: fadeEnd,
    uSquareSize: squareSize,
    uMinBright: minBrightness,
    uTwinkleSpeed: twinkleSpeed,
    uTwinkleStrength: twinkleStrength,
    uIntensity: intensity,
    uAlpha: opacity,
    uSquare: hexToRgb(squareColor),
    uBg: hexToRgb(backgroundColor),
  });
</script>

<ShaderCanvas
  fragment={FRAGMENT_SHADER}
  {uniforms}
  {backgroundColor}
  {dpr}
  class={className}
  {children}
/>
