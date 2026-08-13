import fs from "node:fs";
import path from "node:path";
import ts from "typescript";

const sourceRoot = path.resolve("src");
const dictionaryPath = path.join(sourceRoot, "i18n.tsx");
const dictionarySource = fs.readFileSync(dictionaryPath, "utf8");
const dictionaryTree = ts.createSourceFile(dictionaryPath, dictionarySource, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
const russianKeys = new Set();

function collectDictionary(node) {
  if (ts.isVariableDeclaration(node) && node.name.getText(dictionaryTree) === "ru" && node.initializer && ts.isObjectLiteralExpression(node.initializer)) {
    for (const property of node.initializer.properties) {
      if (ts.isPropertyAssignment(property) && (ts.isStringLiteral(property.name) || ts.isNoSubstitutionTemplateLiteral(property.name))) {
        russianKeys.add(property.name.text);
      }
    }
  }
  ts.forEachChild(node, collectDictionary);
}

collectDictionary(dictionaryTree);

function sourceFiles(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const target = path.join(directory, entry.name);
    return entry.isDirectory() ? sourceFiles(target) : /\.tsx?$/.test(entry.name) ? [target] : [];
  });
}

const usedKeys = new Map();
for (const file of sourceFiles(sourceRoot)) {
  if (file === dictionaryPath) continue;
  const source = fs.readFileSync(file, "utf8");
  const tree = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, file.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  function visit(node) {
    if (ts.isCallExpression(node) && node.expression.getText(tree) === "t" && node.arguments.length > 0) {
      const key = node.arguments[0];
      if (ts.isStringLiteral(key) || ts.isNoSubstitutionTemplateLiteral(key)) {
        const location = tree.getLineAndCharacterOfPosition(node.getStart(tree));
        usedKeys.set(key.text, `${path.relative(process.cwd(), file)}:${location.line + 1}`);
      }
    }
    ts.forEachChild(node, visit);
  }
  visit(tree);
}

const missingRussian = [...usedKeys].filter(([key]) => !russianKeys.has(key));
if (missingRussian.length > 0) {
  console.error("Missing Russian translations for English keys:");
  for (const [key, location] of missingRussian) console.error(`  ${location} ${JSON.stringify(key)}`);
  process.exit(1);
}

console.log(`i18n validation passed: ${usedKeys.size} English keys and ${usedKeys.size} Russian translations`);
