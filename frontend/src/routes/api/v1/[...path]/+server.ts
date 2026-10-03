import { dev } from '$app/environment';
import { env } from '$env/dynamic/private';
import type { RequestHandler } from './$types';

import {
  buildMemoBackendFailureResponse,
  fetchMemoBackend,
  MemoBackendTimeoutError
} from '$lib/server/memoBackendRequest';
import {
  allowedMemoProxyMethods,
  buildBackendRequestHeaders,
  buildBackendUrl,
  buildFrontendResponseHeaders,
  InvalidProxyPathError,
  isTrustedMemoProxyRequest,
  MemoProxyBodyTooLargeError,
  readMemoProxyRequestBody
} from '$lib/server/memoProxy';

const DEFAULT_BACKEND_URL = 'http://127.0.0.1:8080';

const proxyRequest: RequestHandler = async ({ request, params, url, fetch, locals }) => {
  const backendUrl = env.BACKEND_URL?.trim() || DEFAULT_BACKEND_URL;
  const developmentUserId = dev ? env.DEVELOPMENT_USER_ID?.trim() || undefined : undefined;
  const method = request.method.toUpperCase();

  let allowedMethods: readonly string[];
  try {
    allowedMethods = allowedMemoProxyMethods(params.path);
  } catch (error) {
    if (error instanceof InvalidProxyPathError) {
      return Response.json({ message: 'Invalid memo API path' }, { status: 400 });
    }
    throw error;
  }

  if (!allowedMethods.includes(method)) {
    return Response.json(
      { message: 'Memo API method not allowed' },
      {
        status: 405,
        headers: {
          Allow: allowedMethods.join(', ')
        }
      }
    );
  }

  if (!isTrustedMemoProxyRequest(method, request.headers, url)) {
    return Response.json({ message: 'Memo mutation request rejected' }, { status: 403 });
  }

  let target: URL;
  try {
    target = buildBackendUrl(backendUrl, params.path, url.search);
  } catch (error) {
    if (error instanceof InvalidProxyPathError) {
      return Response.json({ message: 'Invalid memo API path' }, { status: 400 });
    }

    console.error('Memo backend proxy target is invalid', error);
    return Response.json({ message: 'Memo backend is unavailable' }, { status: 502 });
  }

  let body: ArrayBuffer | undefined;
  if (method !== 'GET' && method !== 'HEAD') {
    try {
      body = await readMemoProxyRequestBody(request);
    } catch (error) {
      if (error instanceof MemoProxyBodyTooLargeError) {
        return Response.json({ message: 'Memo request body is too large' }, { status: 413 });
      }
      throw error;
    }
  }

  const headers = buildBackendRequestHeaders(
    request.headers,
    dev ? { developmentUserId } : { bearerToken: locals.accessToken }
  );

  try {
    const response = await fetchMemoBackend(fetch, target, {
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
    if (error instanceof MemoBackendTimeoutError) {
      console.warn('Memo backend proxy request timed out');
    } else {
      console.error('Memo backend proxy request failed', error);
    }
    return buildMemoBackendFailureResponse(error);
  }
};

export const GET = proxyRequest;
export const POST = proxyRequest;
export const PUT = proxyRequest;
export const PATCH = proxyRequest;
export const DELETE = proxyRequest;
export const OPTIONS = proxyRequest;
export const HEAD = proxyRequest;
