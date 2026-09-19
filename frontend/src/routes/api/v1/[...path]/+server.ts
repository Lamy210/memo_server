import { dev } from '$app/environment';
import { env } from '$env/dynamic/private';
import type { RequestHandler } from './$types';

import {
  buildBackendRequestHeaders,
  buildBackendUrl,
  buildFrontendResponseHeaders
} from '$lib/server/memoProxy';

const DEFAULT_BACKEND_URL = 'http://127.0.0.1:8080';

const proxyRequest: RequestHandler = async ({ request, params, url, fetch }) => {
  const backendUrl = env.BACKEND_URL?.trim() || DEFAULT_BACKEND_URL;
  const developmentUserId = dev ? env.DEVELOPMENT_USER_ID?.trim() || undefined : undefined;
  const target = buildBackendUrl(backendUrl, params.path, url.search);
  const headers = buildBackendRequestHeaders(request.headers, { developmentUserId });
  const method = request.method.toUpperCase();
  const body =
    method === 'GET' || method === 'HEAD'
      ? undefined
      : await request.arrayBuffer().then((value) => (value.byteLength > 0 ? value : undefined));

  try {
    const response = await fetch(target, {
      method,
      headers,
      body,
      redirect: 'manual'
    });

    return new Response(response.body, {
      status: response.status,
      statusText: response.statusText,
      headers: buildFrontendResponseHeaders(response.headers)
    });
  } catch (error) {
    console.error('Memo backend proxy request failed', error);
    return Response.json({ message: 'Memo backend is unavailable' }, { status: 502 });
  }
};

export const GET = proxyRequest;
export const POST = proxyRequest;
export const PUT = proxyRequest;
export const PATCH = proxyRequest;
export const DELETE = proxyRequest;
export const OPTIONS = proxyRequest;
export const HEAD = proxyRequest;
