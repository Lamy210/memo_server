module.exports = {
  ci: {
    collect: {
      numberOfRuns: 3,
      url: [
        'http://127.0.0.1:4173/memos',
        'http://127.0.0.1:4173/memos/new',
        'http://127.0.0.1:4173/memos/018f0c7a-8b7d-7f25-b239-36e6d9f9b001/edit'
      ],
      settings: {
        preset: 'desktop'
      }
    },
    assert: {
      assertions: {
        'categories:performance': ['warn', { minScore: 0.9 }],
        'categories:accessibility': ['error', { minScore: 1 }],
        'categories:best-practices': ['error', { minScore: 0.95 }],
        'categories:seo': ['error', { minScore: 0.95 }]
      }
    },
    upload: {
      target: 'filesystem',
      outputDir: './.lighthouseci-reports'
    }
  }
};
