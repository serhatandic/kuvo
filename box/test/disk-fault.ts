import { spyOn } from "bun:test"
import fs from "node:fs"

/** Observe real fsync calls by filename and optionally inject a descriptor write failure. */
export function interceptSync(fail: (file: string) => boolean, synced: string[] = [], failWrite?: (file: string) => boolean) {
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
  const write = fs.writeSync
  const writeSpy = failWrite && spyOn(fs, "writeSync").mockImplementation((descriptor, ...args) => {
    if (failWrite(paths.get(descriptor) ?? "")) throw Object.assign(new Error("Injected full disk"), { code: "ENOSPC" })
    return Reflect.apply(write, fs, [descriptor, ...args])
  })
  return () => { writeSpy?.mockRestore(); syncSpy.mockRestore(); openSpy.mockRestore() }
}
