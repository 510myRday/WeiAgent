import fs from 'node:fs';
import path from 'node:path';

const frontendRoot = path.resolve(import.meta.dirname, '..');
const sourceRoot = path.join(frontendRoot, 'src');
const localeFiles = {
  zh: path.join(frontendRoot, 'locales', 'zh.json'),
  en: path.join(frontendRoot, 'locales', 'en.json'),
};

function flatten(value, prefix = '', output = new Map()) {
  if (value && typeof value === 'object' && !Array.isArray(value)) {
    for (const [key, child] of Object.entries(value)) {
      flatten(child, prefix ? `${prefix}.${key}` : key, output);
    }
  } else {
    output.set(prefix, value);
  }
  return output;
}

function collectSourceFiles(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const fullPath = path.join(directory, entry.name);
    if (entry.isDirectory()) return collectSourceFiles(fullPath);
    return /\.(ts|tsx)$/.test(entry.name) ? [fullPath] : [];
  });
}

const localeData = Object.fromEntries(Object.entries(localeFiles).map(([locale, file]) => [
  locale,
  JSON.parse(fs.readFileSync(file, 'utf8')),
]));
const flattened = Object.fromEntries(Object.entries(localeData).map(([locale, data]) => [locale, flatten(data)]));
const zhKeys = new Set(flattened.zh.keys());
const enKeys = new Set(flattened.en.keys());
const onlyZh = [...zhKeys].filter((key) => !enKeys.has(key)).sort();
const onlyEn = [...enKeys].filter((key) => !zhKeys.has(key)).sort();

const sourceFiles = collectSourceFiles(sourceRoot);
const sourceText = sourceFiles.map((file) => fs.readFileSync(file, 'utf8')).join('\n');
const exactReferences = new Set();
for (const key of zhKeys) {
  if (sourceText.includes(`'${key}'`) || sourceText.includes(`"${key}"`) || sourceText.includes(`\`${key}\``)) {
    exactReferences.add(key);
  }
}

const dynamicPrefixes = new Set();
for (const match of sourceText.matchAll(/\bt\(\s*`([^$`]+)\$\{/g)) {
  dynamicPrefixes.add(match[1]);
}
const dynamicReferences = new Set([...zhKeys].filter((key) => (
  [...dynamicPrefixes].some((prefix) => key.startsWith(prefix))
)));
const usedKeys = new Set([...exactReferences, ...dynamicReferences]);
const unusedKeys = [...zhKeys].filter((key) => !usedKeys.has(key)).sort();

const duplicateValues = new Map();
for (const [key, value] of flattened.zh) {
  if (typeof value !== 'string' || !value.trim()) continue;
  const keys = duplicateValues.get(value) ?? [];
  keys.push(key);
  duplicateValues.set(value, keys);
}
const duplicateGroups = [...duplicateValues.entries()]
  .filter(([, keys]) => keys.length > 1)
  .sort(([left], [right]) => left.localeCompare(right));

globalThis.console.log(`i18n keys: zh=${zhKeys.size}, en=${enKeys.size}`);
globalThis.console.log(`source coverage: exact=${exactReferences.size}, dynamic=${dynamicReferences.size}`);
globalThis.console.log(`duplicate value groups: ${duplicateGroups.length}`);

if (onlyZh.length || onlyEn.length || unusedKeys.length) {
  if (onlyZh.length) globalThis.console.error(`Only in zh.json:\n${onlyZh.join('\n')}`);
  if (onlyEn.length) globalThis.console.error(`Only in en.json:\n${onlyEn.join('\n')}`);
  if (unusedKeys.length) globalThis.console.error(`Unused translation keys:\n${unusedKeys.join('\n')}`);
  globalThis.process.exitCode = 1;
}

if (duplicateGroups.length) {
  globalThis.console.log('Duplicate values are reported only; keys are kept separate when their page semantics differ.');
}
