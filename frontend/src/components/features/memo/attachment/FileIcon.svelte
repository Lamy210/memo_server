<!-- src/components/features/memo/attachment/FileIcon.svelte -->
<script lang="ts">
    import type { FileType } from '@/lib/utils/mime';
    import { getFileType } from '@/lib/utils/mime';
  
    export let mimeType: string;
    export let size: 'sm' | 'md' | 'lg' = 'md';
  
    const sizes = {
      sm: 'w-4 h-4',
      md: 'w-6 h-6',
      lg: 'w-8 h-8'
    };
  
    const fileType = getFileType(mimeType);
  
    const iconMap: Record<FileType, string> = {
      image: 'image',
      pdf: 'picture_as_pdf',
      text: 'description',
      code: 'code',
      video: 'videocam',
      audio: 'audiotrack',
      other: 'insert_drive_file'
    };
  
    const colorMap: Record<FileType, string> = {
      image: 'text-green-600',
      pdf: 'text-red-600',
      text: 'text-blue-600',
      code: 'text-purple-600',
      video: 'text-yellow-600',
      audio: 'text-pink-600',
      other: 'text-gray-600'
    };
  </script>
  
  <span class="material-icons-outlined {sizes[size]} {colorMap[fileType]}">
    {iconMap[fileType]}
  </span>