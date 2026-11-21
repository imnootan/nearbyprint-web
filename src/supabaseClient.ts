import { createClient } from '@supabase/supabase-js'

export const SUPABASE_URL = 'https://xxabrfeoskaiwhldqhyc.supabase.co'
export const SUPABASE_KEY = 'sb_publishable_EPME-m5k1dF6n8atFFlpXQ_Dx4UVNtb'

export const supabase = createClient(SUPABASE_URL, SUPABASE_KEY, {
  auth: {
    persistSession: true,
    autoRefreshToken: true,
  },
  realtime: {
    params: {
      eventsPerSecond: 10,
    },
  },
})

export type ShopRecord = {
  id?: string
  shop_unique_id: string
  shop_name: string
  owner_name: string
  created_at?: string
}

export type CloudPrintJob = {
  id: string
  shop_unique_id: string
  customer_name: string
  file_url: string
  status: 'pending' | 'printed' | 'rejected' | 'processing'
  created_at?: string
}

/**
 * Extracts the storage relative path (e.g. "nootan_prints/171234_doc.pdf")
 * from a Supabase public storage URL.
 */
export function getStoragePathFromUrl(fileUrl: string): string {
  try {
    const u = new URL(fileUrl)
    const marker = '/temp_docs/'
    const idx = u.pathname.indexOf(marker)
    if (idx !== -1) {
      return decodeURIComponent(u.pathname.substring(idx + marker.length))
    }
    // Fallback: take last segment
    const segments = u.pathname.split('/')
    return decodeURIComponent(segments.slice(-2).join('/'))
  } catch {
    return ''
  }
}

/**
 * Automatically deletes the file from temp_docs storage bucket.
 */
export async function deleteTempDoc(fileUrl: string): Promise<boolean> {
  const path = getStoragePathFromUrl(fileUrl)
  if (!path) return false
  try {
    const { error } = await supabase.storage.from('temp_docs').remove([path])
    if (error) {
      console.warn('Failed to delete temp doc from Supabase:', error)
      return false
    }
    return true
  } catch (err) {
    console.warn('Error deleting temp doc:', err)
    return false
  }
}
