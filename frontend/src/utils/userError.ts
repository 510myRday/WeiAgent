type UserErrorOptions = {
  summary: string;
  recovery?: string;
  includeDetail?: boolean;
};

export function userErrorMessage(
  cause: unknown,
  { summary, recovery, includeDetail = true }: UserErrorOptions,
): string {
  const detail = includeDetail && cause instanceof Error
    ? cause.message.trim()
    : '';
  const stableMessage = [summary.trim(), recovery?.trim()].filter(Boolean).join(' ');

  if (!detail || detail === summary.trim() || stableMessage.includes(detail)) {
    return stableMessage;
  }

  return `${stableMessage} (${detail})`;
}
