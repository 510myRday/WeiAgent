export function displayPath(path: string): string {
  const value = path.trim();
  const uncPrefix = '\\\\?\\UNC\\';
  const extendedPrefix = '\\\\?\\';
  if (value.startsWith(uncPrefix)) return `\\\\${value.slice(uncPrefix.length)}`;
  if (value.startsWith(extendedPrefix)) return value.slice(extendedPrefix.length);
  return value;
}
