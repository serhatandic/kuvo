import { spyOn } from "bun:test"
import fs from "node:fs"

/** Intercept real fsync calls by filename while leaving all other disk writes in place. */
export function interceptSync(fail: (file: string) => boolean, synced: string[] = []) {
  const open = fs.openSync
  const fsync = fs.fsyncSync
  const paths = new Map<number, string>()
  const openSpy = spyOn(fs, "openSync").mockImplementation((file, flags, mode) => {
    const descriptor = open(file, flags, mode)
    paths.set(descriptor, String(file))
    return descriptor
  })
  const syncSpy = spyOn(fs, "fsyncSync").mockImplementation(descriptor => {
    const file = paths.get(descriptor) ?? ""
    if (fail(file)) throw new Error("Injected sync failure")
    fsync(descriptor)
    synced.push(file)
  })
  return () => { syncSpy.mockRestore(); openSpy.mockRestore() }
}
