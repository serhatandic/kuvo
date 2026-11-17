// Echo the bundle version when a scripted peer replies. Tests may explicitly replay an older one.
export function scriptedMoves(link, peer) {
  const bundles = new Map();
  const sent = move => {
    if (move.op === "offer" && move.generation !== undefined) {
      bundles.set(move.id, { generation: move.generation, hash: move.hash });
    }
  };
  const say = move => {
    const previous = bundles.get(move.id);
    if (move.op === "offer") {
      move = { generation: previous ? previous.generation + Number(previous.hash !== move.hash) : 1, ...move };
      sent(move);
    }
    return link.takeMoveMail(peer, { ...bundles.get(move.id), ...move });
  };
  return { sent, say };
}

// Most tests advance a steady clock. A clock-jump test can give keepMoving the clocks separately.
export const moveTime = (link, now = Date.now()) => link.keepMoving(now, performance.now() + Math.max(0, now - Date.now()));
