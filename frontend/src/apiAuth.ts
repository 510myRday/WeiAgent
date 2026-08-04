export function apiAuthorizationHeaders(): Record<string, string> {
  const token = apiAuthorizationToken();
  return token ? { Authorization: `Bearer ${token}` } : {};
}

export function apiAuthorizationToken(): string {
  const buildToken = import.meta.env.VITE_WEIAGENT_API_TOKEN as string | undefined;
  if (buildToken) return buildToken;
  return document
    .querySelector<HTMLMetaElement>('meta[name="weiagent-api-token"]')
    ?.content.trim() ?? '';
}
