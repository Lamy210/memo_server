// src/lib/constants/editorActions.ts
import type { ToolbarGroup } from '../types/editor';

export const toolbarGroups: ToolbarGroup[] = [
  {
    id: 'text',
    label: 'テキスト書式',
    actions: [
      {
        id: 'bold',
        icon: 'format_bold',
        label: '太字',
        shortcut: 'Ctrl+B',
        execute: (text) => `**${text}**`
      },
      {
        id: 'italic',
        icon: 'format_italic',
        label: '斜体',
        shortcut: 'Ctrl+I',
        execute: (text) => `*${text}*`
      },
      {
        id: 'strikethrough',
        icon: 'format_strikethrough',
        label: '取り消し線',
        execute: (text) => `~~${text}~~`
      }
    ]
  },
  {
    id: 'blocks',
    label: 'ブロック要素',
    actions: [
      {
        id: 'heading',
        icon: 'title',
        label: '見出し',
        shortcut: 'Ctrl+H',
        execute: (text) => `\n## ${text}\n`
      },
      {
        id: 'quote',
        icon: 'format_quote',
        label: '引用',
        execute: (text) => text.split('\n').map(line => `> ${line}`).join('\n')
      },
      {
        id: 'code',
        icon: 'code',
        label: 'コードブロック',
        execute: (text) => `\n\`\`\`\n${text}\n\`\`\`\n`
      }
    ]
  },
  {
    id: 'lists',
    label: 'リスト',
    actions: [
      {
        id: 'bullet_list',
        icon: 'format_list_bulleted',
        label: '箇条書き',
        execute: (text) => text.split('\n').map(line => `- ${line}`).join('\n')
      },
      {
        id: 'number_list',
        icon: 'format_list_numbered',
        label: '番号付きリスト',
        execute: (text) => text.split('\n').map((line, i) => `${i + 1}. ${line}`).join('\n')
      },
      {
        id: 'task_list',
        icon: 'check_box',
        label: 'タスクリスト',
        execute: (text) => text.split('\n').map(line => `- [ ] ${line}`).join('\n')
      }
    ]
  }
];