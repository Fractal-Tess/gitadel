<!--
  A slow descent into a ravine. Each pixel ray-marches a rippled heightfield
  that steepens into walls, and is shaded by how many steps the march needed:
  rays that graze a wall take many and read bright, so the creases light up
  while open air stays dark. Distance fades everything into the void.

  The shader is React Bits Pro's Ravine, ported to raw WebGL 1.
-->
<script lang="ts">
  import type { Snippet } from "svelte";

  import ShaderCanvas, {
    hexToRgb,
  } from "$lib/components/app/shader-canvas.svelte";

  interface Props {
    speed?: number;
    /** Ray-march iterations, 32 to 256. */
    steps?: number;
    /** Fraction of the distance-to-surface taken each step. */
    stepScale?: number;
    /** Terrain feature frequency; lower is broader. */
    scale?: number;
    height?: number;
    /** How quickly the ground rises into walls away from the centre line. */
    spread?: number;
    wallCurve?: number;
    /** Distance at which the picture has faded out. 0 disables the fade. */
    fade?: number;
    cameraHeight?: number;
    tilt?: number;
    roll?: number;
    fov?: number;
    /** Colour of open air and faded distance. */
    nearColor?: string;
    /** Colour of the brightest creases. */
    farColor?: string;
    brightness?: number;
    contrast?: number;
    /** Ray jitter that breaks up banding. */
    grain?: number;
    dpr?: number;
    renderScale?: number;
    /** Clock offset, so the still frame under reduced motion is a good one. */
    timeOffset?: number;
    class?: string;
    children?: Snippet;
  }

  let {
    speed = 1,
    steps = 128,
    stepScale = 0.5,
    scale = 0.25,
    height = 1,
    spread = 34,
    wallCurve = 2.5,
    fade = 35,
    cameraHeight = 6,
    tilt = 0.05,
    roll = 0.075,
    fov = 1,
    nearColor = "#000000",
    farColor = "#ffffff",
    brightness = 0.8,
    contrast = 1,
    grain = 0.005,
    dpr = 1,
    renderScale = 1,
    timeOffset = 0,
    class: className = "",
    children,
  }: Props = $props();

  const FRAGMENT_SHADER = `
    precision highp float;

    #define MAX_STEPS 256
    #define HIT_EPSILON 0.001

    varying vec2 vUv;

    uniform vec2 uRes;
    uniform float uTime;
    uniform float uSpeed;
    uniform float uSteps;
    uniform float uStepScale;
    uniform float uScale;
    uniform float uHeight;
    uniform float uSpread;
    uniform float uWallCurve;
    uniform float uFade;
    uniform float uCameraHeight;
    uniform float uTilt;
    uniform float uRoll;
    uniform float uFov;
    uniform vec3 uNear;
    uniform vec3 uFar;
    uniform float uBrightness;
    uniform float uContrast;
    uniform float uGrain;

    const mat2 OCTAVE_TWIST = mat2(0.8, 0.6, -0.6, 0.8);

    mat2 rotate2(float angle) {
      float s = sin(angle);
      float c = cos(angle);
      return mat2(c, -s, s, c);
    }

    float ripple(vec2 p) {
      return sin(1.5 * p.x) * sin(1.5 * p.y);
    }

    void octave(inout vec2 p, inout float sum, float amplitude, float zoom) {
      sum += amplitude * (0.5 + 0.5 * ripple(p));
      p = OCTAVE_TWIST * p * zoom;
    }

    float terrainNoise(vec2 p) {
      float sum = 0.0;
      octave(p, sum, 0.5, 2.02);
      octave(p, sum, 0.25, 2.03);
      octave(p, sum, 0.125, 2.01);
      octave(p, sum, 0.0625, 2.04);
      sum += 0.015625 * (0.5 + 0.5 * ripple(p));
      return sum / 0.96875;
    }

    float hash(vec2 p) {
      return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
    }

    float canyon(vec3 p, float time) {
      vec3 q = p + vec3(0.0, 0.0, time);
      float relief = terrainNoise(q.xz * uScale) * uHeight;
      float wall = pow(abs(q.x) * uSpread, uWallCurve) * 0.0000125;
      return max(q.y - relief * wall, 0.0);
    }

    void main() {
      vec2 uv = (vUv * uRes - 0.5 * uRes) / uRes.y;
      float time = uTime * uSpeed * 2.5;

      vec3 origin = vec3(uv + vec2(0.0, uCameraHeight), -1.0);
      vec3 dir = normalize(vec3(uv * uFov, 1.0));
      dir.zy = rotate2(uTilt) * dir.zy;
      dir.xy = rotate2(uRoll) * dir.xy;

      vec3 p = origin;
      float taken = 0.0;
      for (int i = 0; i < MAX_STEPS; i++) {
        if (float(i) >= uSteps) break;
        taken = float(i);
        float jitter = (hash(p.xz) - 0.5) * uGrain;
        float d = canyon(p + vec3(jitter), time);
        if (d < HIT_EPSILON) break;
        p += dir * d * uStepScale;
      }

      float tone = taken / float(MAX_STEPS);
      tone = (tone - 0.5) * uContrast + 0.5;
      tone = clamp(tone * uBrightness, 0.0, 1.0);
      if (uFade > 0.0) {
        float travelled = distance(origin, p) / uFade;
        tone *= exp(-travelled * travelled);
      }

      gl_FragColor = vec4(mix(uNear, uFar, tone), 1.0);
    }
  `;

  const clamp = (value: number, min: number, max: number) =>
    Math.min(max, Math.max(min, value));

  const uniforms = $derived({
    uSpeed: speed,
    uSteps: Math.round(clamp(steps, 32, 256)),
    uStepScale: clamp(stepScale, 0.1, 1),
    uScale: Math.max(scale, 0.01),
    uHeight: height,
    uSpread: Math.max(spread, 0),
    uWallCurve: Math.max(wallCurve, 0.5),
    uFade: Math.max(fade, 0),
    uCameraHeight: cameraHeight,
    uTilt: tilt,
    uRoll: roll,
    uFov: Math.max(fov, 0.1),
    uNear: hexToRgb(nearColor),
    uFar: hexToRgb(farColor),
    uBrightness: Math.max(brightness, 0),
    uContrast: Math.max(contrast, 0),
    uGrain: Math.max(grain, 0),
  });
</script>

<ShaderCanvas
  fragment={FRAGMENT_SHADER}
  {uniforms}
  backgroundColor={nearColor}
  {dpr}
  {renderScale}
  {timeOffset}
  class={className}
  {children}
/>
