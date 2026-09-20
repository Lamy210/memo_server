export const FRONTEND_SECURITY_HEADERS = {
  'Cross-Origin-Opener-Policy': 'same-origin',
  'Cross-Origin-Resource-Policy': 'same-origin',
  'Permissions-Policy': 'camera=(), geolocation=(), microphone=(), payment=(), usb=()',
  'Referrer-Policy': 'strict-origin-when-cross-origin',
  'X-Content-Type-Options': 'nosniff',
  'X-Frame-Options': 'DENY',
  'X-Permitted-Cross-Domain-Policies': 'none'
} as const;

export function applyFrontendSecurityHeaders(headers: Headers): void {
  for (const [name, value] of Object.entries(FRONTEND_SECURITY_HEADERS)) {
    headers.set(name, value);
  }
}
