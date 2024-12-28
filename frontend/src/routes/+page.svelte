<!-- src/routes/+page.svelte -->
<script lang="ts">
    import SearchBar from '@/components/features/home/SearchBar.svelte';
    import StatsSummary from '@/components/features/home/StatsSummary.svelte';
    import RecentMemos from '@/components/features/home/RecentMemos.svelte';
    import { Button } from '@/components/ui';
  </script>
  
  <div class="space-y-8">
    <div class="text-center mb-12">
      <h1 class="text-4xl font-bold text-gray-900 mb-4">
        メモを始めましょう
      </h1>
      <p class="text-lg text-gray-600 mb-6">
        アイデアや思考を整理し、共有しましょう
      </p>
      <div class="flex justify-center gap-4">
        <Button href="/memos/new" size="lg">
          新規メモを作成
        </Button>
        <Button href="/memos" variant="outline" size="lg">
          メモ一覧を見る
        </Button>
      </div>
    </div>
  
    <SearchBar />
    
    <StatsSummary />
    
    <RecentMemos />
  </div>