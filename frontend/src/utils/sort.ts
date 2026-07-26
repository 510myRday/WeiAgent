const nameCollator = new Intl.Collator('zh-CN', { numeric: true, sensitivity: 'base' });

export function compareNames(left: string, right: string): number {
  return nameCollator.compare(left, right);
}

export function sortByName<T extends { name: string }>(items: readonly T[]): T[] {
  return [...items].sort((a, b) => compareNames(a.name, b.name));
}
