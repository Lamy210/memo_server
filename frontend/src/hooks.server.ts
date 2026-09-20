import type { Handle } from '@sveltejs/kit';

import { applyFrontendSecurityHeaders } from '$lib/server/securityHeaders';

export const handle: Handle = async ({ event, resolve }) => {
  const response = await resolve(event);
  applyFrontendSecurityHeaders(response.headers);
  return response;
};
