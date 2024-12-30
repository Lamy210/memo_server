// src/routes/settings/+page.svelte
<script lang="ts">
  import { Card } from '@/components/ui';

  const sections = [
    {
      title: 'プロフィール設定',
      description: 'ユーザー情報の設定',
      status: '開発中'
    },
    {
      title: '通知設定',
      description: '通知の受信設定',
      status: '開発中'
    },
    {
      title: 'セキュリティ設定',
      description: 'パスワードとセキュリティの設定',
      status: '開発中'
    },
    {
      title: '表示設定',
      description: 'テーマと表示オプション',
      status: '開発中'
    }
  ];
</script>

<div class="space-y-6">
  <h1 class="text-2xl font-bold text-gray-900">設定</h1>

  <div class="grid gap-6 grid-cols-1 md:grid-cols-2">
    {#each sections as section}
      <Card padding="lg" hover={true}>
        <div class="space-y-2">
          <h2 class="text-lg font-medium text-gray-900">
            {section.title}
          </h2>
          <p class="text-gray-500">
            {section.description}
          </p>
          <div class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-yellow-100 text-yellow-800">
            {section.status}
          </div>
        </div>
      </Card>
    {/each}
  </div>
</div>