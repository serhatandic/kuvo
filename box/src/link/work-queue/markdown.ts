import { fromMarkdown } from "mdast-util-from-markdown";
import type { Nodes as Node } from "mdast";
export interface SourceRange { start: number; end: number }

// The parser reports offsets into the original JS string, including both halves of surrogate pairs.
export function markdownRanges(text: string, select: (node: Node) => boolean): (SourceRange & { node: Node })[] {
  const ranges: (SourceRange & { node: Node })[] = [];
  const visit = (node: Node) => {
    if (select(node)) {
      const start = node.position?.start.offset, end = node.position?.end.offset;
      if (start !== undefined && end !== undefined) ranges.push({ start, end, node });
    } else if ("children" in node) {
      node.children.forEach(visit);
    }
  };
  visit(fromMarkdown(text));
  return ranges;
}

export const protectedRanges = (text: string) => markdownRanges(text, node =>
  ["code", "inlineCode", "link", "linkReference", "image", "imageReference", "definition"].includes(node.type));

/** Remove only the requested picture markers from the current description. */
export function withoutPictures(text: string, urls: ReadonlySet<string>): string {
  if (!urls.size) return text;
  const ranges: SourceRange[] = markdownRanges(text, node => node.type === "image" && urls.has(node.url));
  if (text.includes("[Image ") && [...urls].some(url => url.startsWith("kuvo-attachment:legacy/"))) {
    const protectedText = protectedRanges(text);
    for (const match of text.matchAll(/\[Image ([0-9]{1,3})\]/g)) {
      const start = match.index, end = start + match[0].length;
      if (!urls.has(`kuvo-attachment:legacy/${match[1]}`) || text[start - 1] === "\\" ||
          protectedText.some(range => range.start < end && start < range.end)) continue;
      ranges.push({ start, end });
    }
  }
  ranges.sort((a, b) => b.start - a.start);
  for (const { start, end } of ranges) text = text.slice(0, start) + text.slice(end);
  return text;
}
