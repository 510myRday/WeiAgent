export function displayPath(path: string): string {
  const value = path.trim();
  const uncPrefix = '\\\\?\\UNC\\';
  const extendedPrefix = '\\\\?\\';
  const slashUncPrefix = '//?/UNC/';
  const slashExtendedPrefix = '//?/';
  if (value.startsWith(uncPrefix)) return `\\\\${value.slice(uncPrefix.length)}`;
  if (value.startsWith(extendedPrefix)) return value.slice(extendedPrefix.length);
  if (value.startsWith(slashUncPrefix)) return `//${value.slice(slashUncPrefix.length)}`;
  if (value.startsWith(slashExtendedPrefix)) return value.slice(slashExtendedPrefix.length);
  return value;
}

export function displaySkillFilePath(path: string, rootDir: string): string {
  const normalizedPath = displayPath(path).replace(/\\/g, '/').replace(/\/+$/, '');
  const normalizedRoot = displayPath(rootDir).replace(/\\/g, '/').replace(/\/+$/, '');

  if (normalizedRoot && normalizedPath.toLowerCase().startsWith(`${normalizedRoot.toLowerCase()}/`)) {
    return normalizedPath.slice(normalizedRoot.length + 1) || 'SKILL.md';
  }
  return normalizedPath || 'SKILL.md';
}
