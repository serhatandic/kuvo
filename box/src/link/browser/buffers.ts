// Chunk queues keep large uploads linear: no copying the accumulated payload on every read.
export class ChunkQueue {
  private chunks: Buffer[] = [];
  private index = 0;
  private offset = 0;
  length = 0;
  append(chunk: Buffer): void { if (chunk.length) { this.chunks.push(chunk); this.length += chunk.length; } }
  peek(size: number): Buffer {
    const out = Buffer.allocUnsafe(size);
    let at = 0, index = this.index, offset = this.offset;
    while (at < size) {
      const chunk = this.chunks[index++]!;
      const count = Math.min(size - at, chunk.length - offset);
      chunk.copy(out, at, offset, offset + count); at += count; offset = 0;
    }
    return out;
  }
  drop(size: number): void {
    this.length -= size;
    while (size) {
      const chunk = this.chunks[this.index]!, count = Math.min(size, chunk.length - this.offset);
      size -= count; this.offset += count;
      if (this.offset === chunk.length) { this.index++; this.offset = 0; }
    }
    if (this.index > 64 && this.index * 2 >= this.chunks.length) { this.chunks = this.chunks.slice(this.index); this.index = 0; }
  }
  read(size: number): Buffer { const out = this.peek(size); this.drop(size); return out; }
}
export class LineBuffer {
  private chunks: Buffer[] = [];
  private size = 0;
  constructor(private limit: number) {}
  feed(chunk: Buffer, line: (text: string) => void): void {
    let start = 0;
    while (start < chunk.length) {
      const at = chunk.indexOf(10, start), end = at < 0 ? chunk.length : at;
      const piece = chunk.subarray(start, end);
      this.size += piece.length;
      if (this.size > this.limit) throw new Error("Browser message too large.");
      this.chunks.push(piece);
      if (at >= 0) {
        const text = Buffer.concat(this.chunks, this.size).toString("utf8");
        this.chunks = []; this.size = 0; line(text);
      }
      start = end + 1;
    }
  }
}
