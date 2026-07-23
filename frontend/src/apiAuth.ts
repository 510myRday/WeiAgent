export function apiAuthorizationHeaders(): Record<string, string> {
  const token = apiToken();
  return token ? { Authorization: `Bearer ${token}` } : {};
}

function apiToken(): string {
  const buildToken = import.meta.env.VITE_WEIAGENT_API_TOKEN as string | undefined;
  if (buildToken) return buildToken;
  return document
    .querySelector<HTMLMetaElement>('meta[name="weiagent-api-token"]')
    ?.content.trim() ?? '';
}
