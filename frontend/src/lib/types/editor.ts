// src/lib/types/editor.ts
export interface ToolbarAction {
    id: string;
    icon: string;
    label: string;
    shortcut?: string;
    execute: (selection: string) => string;
  }
  
  export interface ToolbarGroup {
    id: string;
    label: string;
    actions: ToolbarAction[];
  }