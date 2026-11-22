// State shared with code that must not import the session follower's dependency graph.
let keeping = false;

export const keepsList = (): boolean => keeping;
export function setKeepsList(value: boolean): void { keeping = value; }
