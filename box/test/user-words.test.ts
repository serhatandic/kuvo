import "./safety"
import { expect, test } from "bun:test";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const sourceFolder = fileURLToPath(new URL("../src", import.meta.url));
const wireTerms = new Set(["primary", "primary?", "setup.sole-primary"]);

function* sourceFiles(folder: string): Generator<string> {
  for (const entry of fs.readdirSync(folder, { withFileTypes: true })) {
    const file = path.join(folder, entry.name);
    if (entry.isDirectory()) yield* sourceFiles(file);
    else if (/\.[cm]?[jt]s$/.test(entry.name)) yield file;
  }
}

test("box messages call the setup's leading computer the main computer", () => {
  const hits: string[] = [];
  for (const file of sourceFiles(sourceFolder)) {
    const relative = path.relative(sourceFolder, file);
    const source = ts.createSourceFile(file, fs.readFileSync(file, "utf8"), ts.ScriptTarget.Latest, true);
    const visit = (node: ts.Node) => {
      if ((ts.isStringLiteralLike(node) || ts.isTemplateLiteralToken(node)) && !wireTerms.has(node.text)) {
        let words = node.text;
        // (SQL's, and the ownership rules' own role names: never words a person reads.)
        words = words.replace(/\bPRIMARY KEY\b/g, "");
        if (relative === path.join("shared", "mutation-declarations.ts")) words = words.replace(/^current primary$/, "");
        if (relative === "browser/tools.ts") words = words.replace("class primary for the main one", "");
        if (/\bprimary\b/i.test(words)) {
          const { line } = source.getLineAndCharacterOfPosition(node.getStart(source));
          hits.push(`${relative}:${line + 1}: ${JSON.stringify(words)}`);
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(source);
  }
  expect(hits).toEqual([]);
});
