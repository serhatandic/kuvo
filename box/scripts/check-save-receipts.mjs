// TypeScript allows unused results and truthiness checks on objects. Follow shared-save
// results through bindings and reply wrappers until the complete receipt leaves the caller.
import path from "node:path";
import ts from "typescript";

const producers = new Map([
  ["asSave", []], ["bindSave", []], ["stageShared", []], ["changeShared", []],
  ["submitShared", []], ["waitShared", []], ["withSaveOutcome", []],
  ["sharedSaveReply", ["value"]], ["awaitSharedSave", []], ["updateAccess", ["save"]],
  ["askSetupSave", ["receipt"]], ["askSetupOutcome", ["receipt"]],
  ["saveDraft", ["receipt"]], ["saveOutcome", ["receipt"]], ["askLocalSetup", ["receipt"]],
  ["stagePhonePairing", ["save"]], ["authorizePhonePairing", ["save"]], ["prepareRemoval", ["save"]],
  ["revokeDesktop", ["save"]], ["revokeMemberDesktop", ["save"]],
  ["addLabel", ["save"]], ["changeLabel", ["save"]], ["removeLabel", ["save"]],
]);

const config = ts.readConfigFile("test/agents/tsconfig.json", ts.sys.readFile);
const agents = ts.parseJsonConfigFileContent(config.config, ts.sys, "test/agents");
const root = ts.readConfigFile("tsconfig.json", ts.sys.readFile);
const box = ts.parseJsonConfigFileContent(root.config, ts.sys, ".");
const program = ts.createProgram([...new Set([...box.fileNames, ...agents.fileNames])], box.options);
const types = program.getTypeChecker();

function symbol(node) {
  const found = ts.isShorthandPropertyAssignment(node.parent) && node.parent.name === node
    ? types.getShorthandAssignmentValueSymbol(node.parent) : types.getSymbolAtLocation(node);
  return found && found.flags & ts.SymbolFlags.Alias ? types.getAliasedSymbol(found) : found;
}

function visit(node, inspect) {
  inspect(node);
  ts.forEachChild(node, child => visit(child, inspect));
}

const callers = [];
const references = new Map();
for (const file of program.getSourceFiles()) {
  const filename = path.resolve(file.fileName);
  if (file.isDeclarationFile || !filename.startsWith(path.resolve("src") + path.sep)) continue;
  visit(file, node => {
    if (ts.isIdentifier(node)) {
      const key = symbol(node);
      if (!key) return;
      if (!references.has(key)) references.set(key, []);
      references.get(key).push(node);
    }
    if (!ts.isCallExpression(node)) return;
    const declaration = types.getResolvedSignature(node)?.getDeclaration();
    const name = declaration?.name && ts.isIdentifier(declaration.name) ? declaration.name.text : undefined;
    if (producers.has(name) && declaration && /\/(setup\/shared|setup\/access|setup-local|usage\/settings|link\/phones|peers\/manage|link\/uninstall|work-queue\/labels)\.ts$/.test(path.resolve(declaration.getSourceFile().fileName))) {
      callers.push({ node, name, projection: producers.get(name) });
    }
  });
}

function isReceipt(type) {
  const present = types.getNonNullableType(type);
  return ["setup", "epoch", "operation", "proof", "status"].every(member => types.getPropertyOfType(present, member));
}

function carriesReceipt(type, projection, node) {
  if (!projection.length) return isReceipt(type);
  if (type.isUnion()) return type.types.some(part => carriesReceipt(part, projection, node));
  const field = types.getPropertyOfType(type, projection[0]);
  return field && carriesReceipt(types.getTypeOfSymbolAtLocation(field, node), projection.slice(1), node);
}

function keepsBinding(name, projection, seen) {
  if (ts.isObjectBindingPattern(name)) {
    return name.elements.some(element => {
      if (element.dotDotDotToken) return false;
      const property = element.propertyName ?? element.name;
      return ts.isIdentifier(property) && property.text === projection[0]
        && keepsBinding(element.name, projection.slice(1), seen);
    });
  }
  return (references.get(symbol(name)) ?? []).some(use => use !== name
    && (!projection.length || carriesReceipt(types.getTypeAtLocation(use), projection, use))
    && keeps(use, projection, seen));
}

function keeps(node, projection, seen = new Set()) {
  if (seen.has(node)) return false;
  seen.add(node);
  const parent = node.parent;
  if (ts.isVariableDeclaration(parent) && parent.initializer === node) return keepsBinding(parent.name, projection, seen);
  if (ts.isPropertyAccessExpression(parent) && parent.expression === node) {
    return parent.name.text === projection[0] && keeps(parent, projection.slice(1), seen);
  }
  if (ts.isAwaitExpression(parent) || ts.isParenthesizedExpression(parent) || ts.isNonNullExpression(parent)) return keeps(parent, projection, seen);
  if (ts.isAsExpression(parent) && parent.type.getText() === "const") return keeps(parent, projection, seen);
  if (ts.isConditionalExpression(parent) && parent.condition !== node) return keeps(parent, projection, seen);
  if (ts.isBinaryExpression(parent)) {
    const op = parent.operatorToken.kind;
    if (op === ts.SyntaxKind.EqualsToken && parent.right === node) {
      if (ts.isIdentifier(parent.left)) return keepsBinding(parent.left, projection, seen);
      return !projection.length && ts.isPropertyAccessExpression(parent.left) && isReceipt(types.getTypeAtLocation(parent.left));
    }
    if (op === ts.SyntaxKind.QuestionQuestionToken || (op === ts.SyntaxKind.AmpersandAmpersandToken && parent.right === node)
      || op === ts.SyntaxKind.BarBarToken) return keeps(parent, projection, seen);
  }
  if (ts.isReturnStatement(parent) || (ts.isArrowFunction(parent) && parent.body === node)
    || ts.isYieldExpression(parent)) return true;
  if ((ts.isPropertyAssignment(parent) && parent.initializer === node) || ts.isShorthandPropertyAssignment(parent) || ts.isSpreadAssignment(parent)) {
    // The whole receipt is embedded in the caller's reply or durable record.
    return keeps(parent.parent, [], seen);
  }
  if (ts.isArrayLiteralExpression(parent)) return keeps(parent, [], seen);
  if (!ts.isCallExpression(parent) || !parent.arguments.includes(node)) return false;
  const callee = symbol(parent.expression)?.getName();
  if (callee === "stringify" && parent.expression.getText() === "JSON.stringify") return keeps(parent, [], seen);
  // Reply and private-file boundaries intentionally take unknown or bytes.
  if (["json", "answer", "say", "writeDurablePrivate", "withSaveOutcome"].includes(callee)) return true;
  if (parent.expression.getText() === "console.log" && node.getSourceFile().fileName.endsWith("/link/uninstall.ts")) return true;
  const signature = types.getResolvedSignature(parent);
  const parameter = signature?.parameters[parent.arguments.indexOf(node)];
  if (!parameter || !carriesReceipt(types.getTypeOfSymbolAtLocation(parameter, parent), projection, parent)) return false;
  const declaration = parameter.valueDeclaration;
  if (declaration && ts.isParameter(declaration) && declaration.parent.body
    && !declaration.getSourceFile().isDeclarationFile) return keepsBinding(declaration.name, projection, seen);
  // A typed interface method hands the receipt to its implementation.
  return true;
}

let failures = 0;
for (const { node, name, projection } of callers) {
  if (keeps(node, projection)) continue;
  const file = node.getSourceFile();
  const { line } = file.getLineAndCharacterOfPosition(node.getStart());
  console.error(`${file.fileName}:${line + 1}: keep the complete receipt from ${name}`);
  failures++;
}
if (!callers.length) throw new Error("No shared-save callers checked");
if (failures) process.exitCode = 1;
else console.log(`Shared-save receipt handling checked at ${callers.length} call sites.`);
