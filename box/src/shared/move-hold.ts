/** A legacy move hold, or the ID of the move that now owns the hold. */
export function moveHold(value: string | null | undefined): string | null {
  const marker = value?.trim();
  if (marker === "moving") return marker;
  return /^moving:mov_[A-Za-z0-9]{1,40}$/.test(marker ?? "") ? marker!.slice("moving:".length) : null;
}

export const moveMarker = (id: string): string => `moving:${id}`;
