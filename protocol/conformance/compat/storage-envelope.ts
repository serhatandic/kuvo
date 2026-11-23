// Decode the shared envelope layer with the TypeScript reference primitives.
import { readFileSync } from "node:fs";
import { additiveListFormat, objectFormat } from "../../../box/src/shared/format-version";
import { readVersionedFile } from "../../../box/src/shared/versioned-file";

const entries = JSON.parse(readFileSync(0, "utf8"));
console.log(JSON.stringify(entries.map(({ file, selection, version, kind, nested, base64 }) => {
  const value = () => {
    const json = JSON.parse(readFileSync(file, "utf8"));
    const selected = nested ? json[nested] : json;
    return base64 ? JSON.parse(Buffer.from(selected, "base64url").toString("utf8")) : selected;
  };
  switch (kind) {
    case "object": {
      const format = objectFormat(file, version);
      return nested ? format.read(value()) : readVersionedFile(file, format);
    }
    case "list": return additiveListFormat(file).read(value());
    case "lines": return readFileSync(file, "utf8").trimEnd().split("\n").map(line => objectFormat(file, version).read(JSON.parse(line)));
    case "stamp": {
      const fields = value();
      if (fields[selection] !== version) throw new Error("Unexpected stamp in " + file);
      return fields;
    }
    default: throw new Error("Unknown envelope " + kind);
  }
})));
