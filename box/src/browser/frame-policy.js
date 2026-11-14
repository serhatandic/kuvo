// One capture class's clock and digest; pure so its backoff is tested without sleeps.
export function framePolicy() {
  let digest = null, quiet = 0, nextAt = 0, floorAt = 0;
  return {
    due(now) { return now >= nextAt; },
    dirty() { nextAt = floorAt; quiet = 0; },
    captured(image, now, fps) {
      const changed = image !== digest;
      digest = image;
      quiet = changed ? 0 : quiet + 1;
      floorAt = now + 1000 / fps;
      nextAt = now + (changed ? 1000 / fps : Math.min(2000, 125 * 2 ** Math.min(quiet, 4)));
      return changed;
    },
  };
}
