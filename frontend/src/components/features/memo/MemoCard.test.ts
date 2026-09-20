import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';

import type { Memo } from '@/lib/api/types';
import MemoCard from './MemoCard.svelte';

const memo: Memo = {
  id: '018f0c7a-8b7d-7f25-b239-36e6d9f9b001',
  title: 'Architecture notes',
  content: 'Keep frontend behavior observable from a user perspective.',
  tags: ['testing', 'architecture'],
  user_id: '12345678-1234-1234-1234-123456789012',
  created_at: '2026-09-20T00:00:00.000Z',
  updated_at: '2026-09-20T00:00:00.000Z',
  version: 7
};

describe('MemoCard', () => {
  it('renders memo content as a navigable card', () => {
    render(MemoCard, { memo });

    const link = screen.getByRole('link', { name: /Architecture notes/i });

    expect(link.getAttribute('href')).toBe(`/memos/${memo.id}/edit`);
    expect(screen.getByRole('heading', { name: memo.title }).textContent).toBe(memo.title);
    expect(screen.getByText('#testing').textContent).toBe('#testing');
    expect(screen.getByText('#architecture').textContent).toBe('#architecture');
    expect(screen.getByText(/Keep frontend behavior observable/).textContent).toContain(
      'Keep frontend behavior observable'
    );
  });

  it('shows the empty-body fallback without rendering tag pills', () => {
    render(MemoCard, {
      memo: {
        ...memo,
        content: '   ',
        tags: []
      }
    });

    expect(screen.getByText('本文はまだありません').textContent).toBe('本文はまだありません');
    expect(screen.queryByText('#testing')).toBeNull();
  });
});
