// src/lib/stores/attachmentStore.ts
import { writable, derived } from 'svelte/store';
import type { Attachment, UploadProgress } from '../types/attachment';
import * as attachmentApi from '../api/attachment';

interface AttachmentState {
  items: Attachment[];
  uploads: Map<string, UploadProgress>;
  isLoading: boolean;
  error: string | null;
}

function createAttachmentStore() {
  const initialState: AttachmentState = {
    items: [],
    uploads: new Map(),
    isLoading: false,
    error: null
  };

  const { subscribe, set, update } = writable<AttachmentState>(initialState);

  return {
    subscribe,
    
    // 添付ファイル一覧の取得
    async fetchAttachments(memoId: string) {
      update(state => ({ ...state, isLoading: true, error: null }));
      
      try {
        const attachments = await attachmentApi.fetchAttachments(memoId);
        update(state => ({
          ...state,
          items: attachments,
          isLoading: false
        }));
      } catch (error) {
        update(state => ({
          ...state,
          error: '添付ファイルの取得に失敗しました',
          isLoading: false
        }));
      }
    },

    // ファイルアップロード
    async uploadFile(file: File, memoId: string) {
      const fileId = crypto.randomUUID();
      
      update(state => ({
        ...state,
        uploads: new Map(state.uploads).set(fileId, {
          fileId,
          progress: 0,
          status: 'pending'
        })
      }));

      try {
        const attachment = await attachmentApi.uploadFile(file, memoId, {
          onProgress: (progress) => {
            update(state => ({
              ...state,
              uploads: new Map(state.uploads).set(fileId, {
                fileId,
                progress,
                status: 'uploading'
              })
            }));
          }
        });

        update(state => ({
          ...state,
          items: [...state.items, attachment],
          uploads: new Map(state.uploads).set(fileId, {
            fileId,
            progress: 100,
            status: 'completed'
          })
        }));

        return attachment;
      } catch (error) {
        update(state => ({
          ...state,
          uploads: new Map(state.uploads).set(fileId, {
            fileId,
            progress: 0,
            status: 'error',
            error: '添付ファイルのアップロードに失敗しました'
          })
        }));
        throw error;
      }
    },

    // 添付ファイルの削除
    async deleteAttachment(attachmentId: string) {
      try {
        await attachmentApi.deleteAttachment(attachmentId);
        update(state => ({
          ...state,
          items: state.items.filter(item => item.id !== attachmentId)
        }));
      } catch (error) {
        update(state => ({
          ...state,
          error: '添付ファイルの削除に失敗しました'
        }));
        throw error;
      }
    }
  };
}

export const attachments = createAttachmentStore();

// アップロード進捗状況の派生ストア
export const uploadProgress = derived(
  attachments,
  $attachments => Array.from($attachments.uploads.values())
);