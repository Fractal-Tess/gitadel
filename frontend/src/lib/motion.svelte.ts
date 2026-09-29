import { tick } from "svelte";

export type MotionPreference = "system" | "reduce";

/**
 * Whether the interface animates. The account preference can turn motion off
 * outright; otherwise the browser's reduced-motion setting decides. The root
 * layout mirrors `reduced` onto `<html data-motion>`, where one rule in
 * `app.css` stills every CSS animation and transition, so components only
 * consult this for motion that CSS cannot reach.
 */
class Motion {
  preference = $state<MotionPreference>("system");
  /** The browser's `prefers-reduced-motion`, kept current as it changes. */
  systemReduced = $state(false);

  constructor() {
    if (typeof window === "undefined") return;
    const query = window.matchMedia("(prefers-reduced-motion: reduce)");
    this.systemReduced = query.matches;
    query.addEventListener("change", (event) => {
      this.systemReduced = event.matches;
    });
  }

  get reduced(): boolean {
    return this.preference === "reduce" || this.systemReduced;
  }
}

export const motion = new Motion();

/**
 * Runs `update` inside a view transition when motion is allowed and the
 * browser supports one, so the change cross-fades instead of snapping.
 */
export async function withViewTransition(
  update: () => void | Promise<void>,
): Promise<void> {
  if (motion.reduced || !document.startViewTransition) {
    await update();
    return;
  }
  await document.startViewTransition(async () => {
    await update();
    await tick();
  }).updateCallbackDone;
}
