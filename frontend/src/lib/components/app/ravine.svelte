<!--
  A slow flight down an endless canyon. Each pixel ray-marches a ridged
  heightfield. Walls are drawn as thin contour lines that dim with how far the
  ray travelled, and the far end fogs into a low haze.
-->
<script lang="ts">
  import type { Snippet } from "svelte";

  import ShaderCanvas, {
    hexToRgb,
  } from "$lib/components/app/shader-canvas.svelte";

  interface Props {
    /** Forward travel in world units per second. */
    speed?: number;
    lineColor?: string;
    /** Accent mixed into the ridge lines and haze, 0 for monochrome. */
    tintColor?: string;
    tintAmount?: number;
    backgroundColor?: string;
    lineStrength?: number;
    /** Contour lines per world unit of wall height. */
    lineDensity?: number;
    /** Haze density; higher pulls the fog closer. */
    fog?: number;
    /** Vertical offset of the vanishing point, in half-heights. */
    horizon?: number;
    dpr?: number;
    renderScale?: number;
    class?: string;
    children?: Snippet;
  }

  let {
    speed = 0.55,
    lineColor = "#d6d3d1",
    tintColor = "#f97316",
    tintAmount = 0.2,
    backgroundColor = "#050505",
    lineStrength = 1,
    lineDensity = 6,
    fog = 0.075,
    horizon = -0.14,
    dpr = 1,
    renderScale = 1,
    class: className = "",
    children,
  }: Props = $props();

  const FRAGMENT_SHADER = `#extension GL_OES_standard_derivatives : enable
    precision highp float;
    varying vec2 vUv;
    uniform vec2 uRes;
    uniform float uTime;
    uniform float uSpeed;
    uniform float uLineStrength;
    uniform float uLineDensity;
    uniform float uFog;
    uniform float uHorizon;
    uniform float uTintAmount;
    uniform vec3 uLine;
    uniform vec3 uTint;
    uniform vec3 uBg;

    const int STEPS = 80;
    const float FAR = 40.0;

    float hash(vec2 p) {
      p = fract(p * vec2(123.34, 456.21));
      p += dot(p, p + 45.32);
      return fract(p.x * p.y);
    }

    float noise(vec2 p) {
      vec2 i = floor(p);
      vec2 f = fract(p);
      vec2 u = f * f * (3.0 - 2.0 * f);
      return mix(
        mix(hash(i), hash(i + vec2(1.0, 0.0)), u.x),
        mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), u.x),
        u.y
      );
    }

    // Ridged noise: 1.0 along crests, falling off either side.
    float ridge(vec2 p) {
      return 1.0 - abs(noise(p) * 2.0 - 1.0);
    }

    float ridgedFbm(vec2 p) {
      float sum = 0.0;
      float amp = 0.5;
      for (int i = 0; i < 3; i++) {
        float r = ridge(p);
        sum += r * r * amp;
        p = p * 2.07 + vec2(1.7, 9.2);
        amp *= 0.5;
      }
      return sum;
    }

    float pathX(float z) {
      return sin(z * 0.07) * 0.9 + sin(z * 0.031 + 1.3) * 1.2;
    }

    // Features are stretched along z so ridges stream toward the vanishing point.
    vec2 wallCoord(vec3 p) {
      float x = p.x - pathX(p.z);
      return vec2(x * 1.3, p.z * 0.11);
    }

    float height(vec3 p) {
      float x = abs(p.x - pathX(p.z));
      float walls = pow(x, 1.4) * 0.8;
      float ridges = ridgedFbm(wallCoord(p)) * smoothstep(0.1, 1.4, x) * 2.4;
      return walls + ridges - 1.0;
    }

    void main() {
      vec2 uv = (vUv * uRes - 0.5 * uRes) / uRes.y;
      float z = uTime * uSpeed;
      vec3 ro = vec3(pathX(z), -0.35, z);

      // Look ahead along the canyon so the flight follows its bends.
      float yaw = atan(pathX(z + 6.0) - pathX(z), 6.0);
      vec3 rd = normalize(vec3(uv.x, uv.y - uHorizon, 1.3));
      float c = cos(yaw);
      float s = sin(yaw);
      rd.xz = mat2(c, s, -s, c) * rd.xz;

      float t = 0.05;
      float tPrev = t;
      bool hit = false;
      for (int i = 0; i < STEPS; i++) {
        vec3 p = ro + rd * t;
        float d = p.y - height(p);
        if (d < 0.001 * t) {
          hit = true;
          break;
        }
        tPrev = t;
        t += max(d * 0.45, 0.012 * t);
        if (t > FAR) break;
      }
      // Bisect the last step so contour lines do not stair-step.
      if (hit) {
        float lo = tPrev;
        float hi = t;
        for (int i = 0; i < 5; i++) {
          float mid = 0.5 * (lo + hi);
          vec3 p = ro + rd * mid;
          if (p.y - height(p) < 0.0) hi = mid;
          else lo = mid;
        }
        t = hi;
      }

      vec3 lineColor = mix(uLine, uTint, uTintAmount);
      vec3 haze = mix(vec3(0.15), uTint * 0.3, uTintAmount * 0.5);
      float fogMix = 1.0 - exp(-t * uFog);
      vec3 col = uBg;

      if (hit) {
        vec3 p = ro + rd * t;
        // Walls are drawn as their contour lines: level sets of the height
        // field run along the canyon and converge on the vanishing point.
        vec2 q = wallCoord(p);
        float f = (p.y + noise(q * vec2(2.3, 3.1)) * 0.35) * uLineDensity;
        float w = fwidth(f);
        float nearest = abs(fract(f + 0.5) - 0.5);
        float line = 1.0 - smoothstep(0.0, w * 1.25, nearest);
        // Where lines crowd below a pixel apart, fade to their average.
        float resolved = 1.0 - smoothstep(0.25, 0.6, w);
        float ink = mix(0.07, line, resolved);
        float floorFade = smoothstep(0.15, 0.9, abs(p.x - pathX(p.z)));
        float shade = exp(-t * 0.07) * floorFade * uLineStrength;
        col = uBg + lineColor * ink * shade * 0.55;
        col = mix(col, haze, fogMix * 0.8);
      } else {
        // Missed rays: black sky that lifts only near the vanishing point.
        float lift = exp(-max(rd.y + 0.02, 0.0) * 11.0);
        col = mix(col, haze, lift * 0.7);
      }

      // Keep the frame edges quiet so overlaid content stays readable.
      float vignette = smoothstep(1.4, 0.3, length(uv * vec2(0.8, 1.0)));
      col *= mix(0.4, 1.0, vignette);
      gl_FragColor = vec4(col, 1.0);
    }
  `;

  const uniforms = $derived({
    uSpeed: speed,
    uLineStrength: lineStrength,
    uLineDensity: lineDensity,
    uFog: fog,
    uHorizon: horizon,
    uTintAmount: tintAmount,
    uLine: hexToRgb(lineColor),
    uTint: hexToRgb(tintColor),
    uBg: hexToRgb(backgroundColor),
  });
</script>

<ShaderCanvas
  fragment={FRAGMENT_SHADER}
  {uniforms}
  {backgroundColor}
  {dpr}
  {renderScale}
  timeOffset={40}
  class={className}
  {children}
/>
