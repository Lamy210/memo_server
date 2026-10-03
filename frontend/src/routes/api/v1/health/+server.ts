import { json } from '@sveltejs/kit';

import type { RequestHandler } from './$types';

export const GET: RequestHandler = () =>
  json(
    {
      status: 'ok',
      service: 'frontend'
    },
    {
      headers: {
        'Cache-Control': 'no-store'
      }
    }
  );
