import type { IpProxy, IpProxyProtocol } from '../types';

export function displayProxyUrl(value: string) {
  try {
    const url = new URL(value);
    if (url.password) url.password = '****';
    return url.toString().replace(/\/$/, '');
  } catch {
    return value;
  }
}

export function formatProxyLocation(proxy: IpProxy) {
  const location = [proxy.country, proxy.region, proxy.city].filter(Boolean).join(' · ');
  return location || proxy.ip_address || '--';
}

export function proxyProtocolLabel(protocol: IpProxyProtocol) {
  return protocol === 'socks5' ? 'SOCKS5' : protocol.toUpperCase();
}
