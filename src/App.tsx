const IPP_PORT = 8631

import { useState, useEffect, useRef } from 'react'
import {
  Printer, Settings, Wifi, FileText, Check, X, Moon, Sun,
  Copy, AlertTriangle, Loader2, FolderOpen, ChevronRight, RefreshCw, Plus, Send,
  QrCode, Globe, ShieldCheck, ShieldAlert, Network, HelpCircle, Palette, Monitor,
  Cloud, Store, ExternalLink, Volume2, Sparkles, CheckCircle2
} from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { Input } from '@/components/ui/input'
import { invoke } from '@tauri-apps/api/core'
import { listen, UnlistenFn } from '@tauri-apps/api/event'
import { openPath } from '@tauri-apps/plugin-opener'
import { supabase, ShopRecord, CloudPrintJob, deleteTempDoc } from './supabaseClient'

type JobStatus = 'pending' | 'printing' | 'completed' | 'error' | 'canceled'

type PrintJob = {
  id: number | string
  name: string
  device: string
  user: string
  pages: number
  bytes: number
  format: string
  status: JobStatus
  message: string
  printer: string
  file_path?: string
  file_url?: string
  channel?: 'wifi' | 'web' | 'relay' | 'cloud'
  customer?: string
  color_mode?: 'bw' | 'color'
  is_card?: boolean
  cloud_job_id?: string
  created_at?: string
}

type PrinterInfo = {
  name: string
  uri: string
  isSelf: boolean
  online: boolean
}

type RelayStatus = {
  host: string
  url: string
  forwarding: boolean
  warning: string
}

type DiscoveredHost = {
  name: string
  ip: string
  port: number
  host: string
  url: string
}

const statusStyle: Record<JobStatus, string> = {
  pending: 'bg-yellow-500/15 text-yellow-600 dark:text-yellow-400',
  printing: 'bg-sky-500/15 text-sky-600 dark:text-sky-400',
  completed: 'bg-emerald-500/15 text-emerald-600 dark:text-emerald-400',
  error: 'bg-red-500/15 text-red-600 dark:text-red-400',
  canceled: 'bg-muted text-muted-foreground',
}

const statusLabel: Record<JobStatus, string> = {
  pending: 'Pending',
  printing: 'Printing…',
  completed: 'Printed',
  error: 'Failed',
  canceled: 'Rejected',
}

function playDoorbellChime() {
  try {
    const AudioCtx = window.AudioContext || (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext
    if (!AudioCtx) return
    const audioCtx = new AudioCtx()
    if (audioCtx.state === 'suspended') {
      audioCtx.resume()
    }
    const now = audioCtx.currentTime

    // First tone (E5 ~ 659 Hz)
    const osc1 = audioCtx.createOscillator()
    const gain1 = audioCtx.createGain()
    osc1.type = 'sine'
    osc1.frequency.setValueAtTime(659.25, now)
    gain1.gain.setValueAtTime(0.22, now)
    gain1.gain.exponentialRampToValueAtTime(0.001, now + 0.35)
    osc1.connect(gain1)
    gain1.connect(audioCtx.destination)
    osc1.start(now)
    osc1.stop(now + 0.35)

    // Second tone (C5 ~ 523 Hz)
    const osc2 = audioCtx.createOscillator()
    const gain2 = audioCtx.createGain()
    osc2.type = 'sine'
    osc2.frequency.setValueAtTime(523.25, now + 0.16)
    gain2.gain.setValueAtTime(0.22, now + 0.16)
    gain2.gain.exponentialRampToValueAtTime(0.001, now + 0.65)
    osc2.connect(gain2)
    gain2.connect(audioCtx.destination)
    osc2.start(now + 0.16)
    osc2.stop(now + 0.65)
  } catch (e) {
    console.warn('Audio chime warning:', e)
  }
}

function HostOption({
  title,
  subtitle,
  address,
  selected,
  disabled,
  onSelect,
}: {
  title: string
  subtitle?: string
  address?: string
  selected: boolean
  disabled?: boolean
  onSelect: () => void
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onSelect}
      aria-pressed={selected}
      className={`w-full text-left flex items-center gap-3 p-4 transition-colors
        ${selected ? 'bg-primary/10' : 'hover:bg-muted/50'}
        ${disabled ? 'opacity-60 cursor-not-allowed' : ''}`}
    >
      <span
        className={`w-4 h-4 rounded-full border-2 shrink-0 flex items-center justify-center
          ${selected ? 'border-primary' : 'border-muted-foreground/40'}`}
      >
        {selected && <span className="w-2 h-2 rounded-full bg-primary" />}
      </span>
      <span className="flex flex-col gap-0.5 min-w-0 flex-1">
        <span className={`text-sm truncate ${selected ? 'font-medium' : ''}`}>{title}</span>
        {subtitle && <span className="text-xs text-muted-foreground truncate">{subtitle}</span>}
      </span>
      {address && (
        <span className="font-mono text-xs text-muted-foreground shrink-0 select-all">{address}</span>
      )}
    </button>
  )
}

function sanitizePublicUrl(url: string | null | undefined): string {
  if (!url) return 'https://nearbyprint-web.vercel.app'
  const trimmed = url.trim()
  if (!trimmed || trimmed.includes('127.0.0.1') || trimmed.includes('localhost') || trimmed.includes(':1420')) {
    return 'https://nearbyprint-web.vercel.app'
  }
  return trimmed.replace(/\/$/, '')
}

export default function App() {
  // Broadcast & network
  const [isBroadcasting, setIsBroadcasting] = useState(false)
  const [broadcastError, setBroadcastError] = useState('')
  const [serverUrl, setServerUrl] = useState<string>('')
  const [localIps, setLocalIps] = useState<string[]>([])
  const [availablePrinters, setAvailablePrinters] = useState<PrinterInfo[]>([])

  // Dual Printer Selection (B&W vs Color)
  const [selectedBwPrinter, setSelectedBwPrinter] = useState<string>(() => {
    return localStorage.getItem('nearby_selected_bw_printer') || localStorage.getItem('nearby_selected_printer') || ''
  })
  const [selectedColorPrinter, setSelectedColorPrinter] = useState<string>(() => {
    return localStorage.getItem('nearby_selected_color_printer') || ''
  })

  const [isFetchingPrinters, setIsFetchingPrinters] = useState(true)
  const [isRefreshing, setIsRefreshing] = useState(false)

  // Network & IP selection state
  const [selectedIp, setSelectedIp] = useState<string>(() => {
    return localStorage.getItem('nearby_selected_ip') || ''
  })
  const [hostnameUrl, setHostnameUrl] = useState<string>('')
  const [useHostnameQr, setUseHostnameQr] = useState<boolean>(() => {
    return localStorage.getItem('nearby_use_hostname_qr') === 'true'
  })
  const [firewallStatus, setFirewallStatus] = useState<string>('')
  const [isFixingFirewall, setIsFixingFirewall] = useState<boolean>(false)

  // Dual-channel auto-print controls
  const [autoPrintWifi, setAutoPrintWifi] = useState<boolean>(() => {
    return localStorage.getItem('nearby_autoprint_wifi') === 'true'
  })
  const [autoPrintWeb, setAutoPrintWeb] = useState<boolean>(() => {
    return localStorage.getItem('nearby_autoprint_web') === 'true'
  })
  const [autoPrintCloud, setAutoPrintCloud] = useState<boolean>(() => {
    return localStorage.getItem('nearby_autoprint_cloud') === 'true'
  })

  // Permanent SaaS Shop Setup
  const [shopId, setShopId] = useState<string>(() => {
    const saved = localStorage.getItem('nearby_shop_unique_id')
    if (saved === 'nootan_prints' && localStorage.getItem('nearby_shop_configured') !== 'true') return ''
    return saved || ''
  })
  const [shopName, setShopName] = useState<string>(() => {
    const saved = localStorage.getItem('nearby_shop_name')
    if (saved === 'Nootan Prints' && localStorage.getItem('nearby_shop_configured') !== 'true') return ''
    return saved || ''
  })
  const [ownerName, setOwnerName] = useState<string>(() => {
    const saved = localStorage.getItem('nearby_owner_name')
    if (saved === 'Nootan' && localStorage.getItem('nearby_shop_configured') !== 'true') return ''
    return saved || ''
  })
  const [publicAppUrl, setPublicAppUrl] = useState<string>(() => {
    const saved = localStorage.getItem('nearby_public_app_url')
    const sanitized = sanitizePublicUrl(saved)
    if (saved !== sanitized) {
      localStorage.setItem('nearby_public_app_url', sanitized)
    }
    return sanitized
  })
  const [isSavingShop, setIsSavingShop] = useState(false)
  const [shopSaveSuccess, setShopSaveSuccess] = useState(false)
  const [showShopModal, setShowShopModal] = useState<boolean>(() => {
    const savedId = localStorage.getItem('nearby_shop_unique_id')
    const savedName = localStorage.getItem('nearby_shop_name')
    const isConfigured = localStorage.getItem('nearby_shop_configured') === 'true'
    return !savedId || !savedName || !isConfigured
  })
  const [isCloudConnected, setIsCloudConnected] = useState(true)
  const [copiedLink, setCopiedLink] = useState(false)
  const [copiedLocalLink, setCopiedLocalLink] = useState(false)
  const [copiedIpp, setCopiedIpp] = useState(false)

  // Counter View Selection: 'both' | 'wifi' | 'cloud'
  const [counterTab, setCounterTab] = useState<'both' | 'wifi' | 'cloud'>(() => {
    return (localStorage.getItem('nearby_counter_tab') as 'both' | 'wifi' | 'cloud') || 'cloud'
  })
  const [modalQrTab, setModalQrTab] = useState<'wifi' | 'cloud'>('cloud')

  // Queue View Filter: 'all' | 'cloud' | 'wifi'
  const [queueFilter, setQueueFilter] = useState<'all' | 'cloud' | 'wifi'>('all')
  const [supabaseError, setSupabaseError] = useState<string | null>(null)

  const [showQrModal, setShowQrModal] = useState(false)
  const [printStandeeType, setPrintStandeeType] = useState<'cloud' | 'wifi'>('cloud')

  // Settings modal & sub-tab
  const [showSettings, setShowSettings] = useState(false)
  const [settingsTab, setSettingsTab] = useState<'cloud' | 'network' | 'printers' | 'firewall' | 'relay'>('cloud')
  const [deviceName, setDeviceName] = useState<string>('')
  const [isSavingName, setIsSavingName] = useState(false)

  // Local relay
  const [relayHost, setRelayHost] = useState('')
  const [relayStatus, setRelayStatus] = useState<RelayStatus>({ host: '', url: '', forwarding: false, warning: '' })
  const [relayWarning, setRelayWarning] = useState('')
  const [discoveredHosts, setDiscoveredHosts] = useState<DiscoveredHost[]>([])
  const [isSavingRelay, setIsSavingRelay] = useState(false)
  const [relayBusy, setRelayBusy] = useState(false)
  const [relayResult, setRelayResult] = useState<{ ok: boolean; message: string } | null>(null)
  const [proxyError, setProxyError] = useState('')
  const [firewallOk, setFirewallOk] = useState<boolean>(true)

  // Theme & jobs
  const [theme, setTheme] = useState<'light' | 'dark'>(() => {
    return (localStorage.getItem('nearby_theme') as 'light' | 'dark') || 'dark'
  })
  const [jobs, setJobs] = useState<PrintJob[]>([])

  useEffect(() => {
    document.documentElement.classList.toggle('dark', theme === 'dark')
    localStorage.setItem('nearby_theme', theme)
  }, [theme])

  // Dynamic environment detection (Tauri binary vs Web dev preview)
  const isTauri = typeof window !== 'undefined' && Boolean((window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
  const activePort = isTauri ? IPP_PORT : (window.location.port ? Number(window.location.port) : IPP_PORT)

  // Refs for auto-printing without stale closures
  const autoPrintWifiRef = useRef(autoPrintWifi)
  const autoPrintWebRef = useRef(autoPrintWeb)
  const autoPrintCloudRef = useRef(autoPrintCloud)
  const selectedBwPrinterRef = useRef(selectedBwPrinter)
  const selectedColorPrinterRef = useRef(selectedColorPrinter)
  const inFlight = useRef<Set<number | string>>(new Set())

  useEffect(() => {
    autoPrintWifiRef.current = autoPrintWifi
    localStorage.setItem('nearby_autoprint_wifi', String(autoPrintWifi))
  }, [autoPrintWifi])

  useEffect(() => {
    autoPrintWebRef.current = autoPrintWeb
    localStorage.setItem('nearby_autoprint_web', String(autoPrintWeb))
  }, [autoPrintWeb])

  useEffect(() => {
    autoPrintCloudRef.current = autoPrintCloud
    localStorage.setItem('nearby_autoprint_cloud', String(autoPrintCloud))
  }, [autoPrintCloud])

  useEffect(() => {
    selectedBwPrinterRef.current = selectedBwPrinter
    localStorage.setItem('nearby_selected_bw_printer', selectedBwPrinter)
    if (selectedBwPrinter) {
      invoke('set_selected_printer', { printer: selectedBwPrinter }).catch(() => {})
    }
  }, [selectedBwPrinter])

  useEffect(() => {
    selectedColorPrinterRef.current = selectedColorPrinter
    localStorage.setItem('nearby_selected_color_printer', selectedColorPrinter)
  }, [selectedColorPrinter])

  // Resolve target printer based on job color mode
  const getJobPrinter = (colorMode?: 'bw' | 'color'): string => {
    if (colorMode === 'color') {
      return selectedColorPrinterRef.current || selectedBwPrinterRef.current
    }
    return selectedBwPrinterRef.current || selectedColorPrinterRef.current
  }

  // Load printer name & initial relay status on mount
  useEffect(() => {
    invoke<string>('get_printer_name')
      .then(setDeviceName)
      .catch(() => {})
    invoke<RelayStatus>('get_relay_status')
      .then(status => {
        setRelayStatus(status)
        setRelayHost(status.host)
        setRelayWarning(status.warning)
      })
      .catch(() => {})
    invoke<{ running: boolean; error: string }>('get_local_proxy_status')
      .then(status => setProxyError(status.running ? '' : status.error))
      .catch(() => {})
    invoke('start_discovery').catch(e => console.error('Failed to start discovery:', e))
    invoke<DiscoveredHost[]>('get_discovered_hosts')
      .then(setDiscoveredHosts)
      .catch(() => {})
    invoke<boolean>('check_firewall_status')
      .then(setFirewallOk)
      .catch(() => {})
  }, [])

  // Listen for discovered hosts
  useEffect(() => {
    let unlisten: UnlistenFn | undefined
    let cancelled = false
    listen<DiscoveredHost[]>('discovered-hosts', e => {
      if (!cancelled) setDiscoveredHosts(e.payload)
    })
      .then(fn => {
        if (cancelled) fn()
        else unlisten = fn
      })
      .catch(e => console.error('Failed to subscribe to discovered hosts:', e))
    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [])

  // Listen for relay warnings
  useEffect(() => {
    let unlisten: UnlistenFn | undefined
    let cancelled = false
    listen<string>('relay-warning', e => {
      if (!cancelled) setRelayWarning(e.payload)
    })
      .then(fn => {
        if (cancelled) fn()
        else unlisten = fn
      })
      .catch(e => console.error('Failed to subscribe to relay warnings:', e))
    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [])

  // Load network details & IPs on mount
  useEffect(() => {
    invoke<{ ip: string; all_ips: string[]; hostname: string; hostname_url: string }>('get_network_details')
      .then(net => {
        setHostnameUrl(net.hostname_url)
        if (net.all_ips && net.all_ips.length > 0) {
          setLocalIps(net.all_ips)
          if (!selectedIp || !net.all_ips.includes(selectedIp)) {
            const best = net.ip || net.all_ips[0]
            setSelectedIp(best)
            localStorage.setItem('nearby_selected_ip', best)
          }
        }
      })
      .catch(() => {})
  }, [])

  // Supabase Direct Query + 3-Second Polling + Realtime Listener
  useEffect(() => {
    const activeShopId = (shopId || '').trim().toLowerCase()
    if (!activeShopId) {
      setSupabaseError('Shop ID is not configured. Please set your Shop ID in Settings or Edit Shop.')
      return
    }

    setSupabaseError(null)

    // Direct query function: runs immediately and every 3 seconds
    const fetchPendingCloudJobs = async () => {
      try {
        const { data, error } = await supabase
          .from('print_jobs')
          .select('*')
          .eq('shop_unique_id', activeShopId)
          .eq('status', 'pending')
          .order('created_at', { ascending: false })

        if (error) {
          console.error('Supabase query error:', error)
          setSupabaseError(error.message || JSON.stringify(error))
          return
        }

        setSupabaseError(null)

        if (data) {
          // Exact column mapping: id, shop_unique_id, customer_name, file_url, status, created_at
          const pendingCloudJobs: PrintJob[] = data.map((cj: any) => {
            const fileUrl = cj.file_url || ''
            const customerName = cj.customer_name || 'Customer'
            const fileName = fileUrl ? (fileUrl.split('/').pop()?.split('?')[0] || 'document.pdf') : 'document.pdf'
            const isColor = /color/i.test(customerName)
            return {
              id: `cloud_${cj.id}`,
              cloud_job_id: String(cj.id),
              name: decodeURIComponent(fileName),
              device: `Cloud: ${customerName}`,
              user: customerName,
              pages: 1,
              bytes: 0,
              format: 'pdf',
              status: 'pending' as JobStatus,
              message: `Cloud Order from ${customerName}`,
              printer: '',
              file_url: fileUrl,
              channel: 'cloud' as const,
              customer: customerName,
              color_mode: isColor ? ('color' as const) : ('bw' as const),
              created_at: cj.created_at,
            }
          })

          setJobs(prev => {
            const wifiJobs = prev.filter(j => j.channel !== 'cloud')
            const localProcessedCloudJobs = prev.filter(j => j.channel === 'cloud' && j.status !== 'pending')
            const localProcessedIds = new Set(localProcessedCloudJobs.map(j => j.cloud_job_id))

            const activePendingCloud = pendingCloudJobs.filter(j => !localProcessedIds.has(j.cloud_job_id))

            const prevPendingIds = new Set(prev.filter(j => j.channel === 'cloud' && j.status === 'pending').map(j => j.cloud_job_id))
            const newJobs = activePendingCloud.filter(j => !prevPendingIds.has(j.cloud_job_id))

            if (newJobs.length > 0 && prev.length > 0) {
              playDoorbellChime()
            }

            if (autoPrintCloudRef.current) {
              newJobs.forEach(job => {
                if (!inFlight.current.has(job.id)) {
                  handleAcceptCloudJob(job)
                }
              })
            }

            return [...activePendingCloud, ...localProcessedCloudJobs, ...wifiJobs]
          })
        }
      } catch (err: any) {
        console.error('Supabase query exception:', err)
        setSupabaseError(err?.message || String(err))
      }
    }

    // 1. Initial direct query on load
    fetchPendingCloudJobs()

    // 2. 3-Second interval polling
    const pollInterval = setInterval(fetchPendingCloudJobs, 3000)

    // 3. Realtime subscription for instantaneous response
    const channel = supabase
      .channel(`realtime-shop-${activeShopId}`)
      .on(
        'postgres_changes',
        {
          event: '*',
          schema: 'public',
          table: 'print_jobs',
          filter: `shop_unique_id=eq.${activeShopId}`,
        },
        payload => {
          if (payload.eventType === 'INSERT') {
            fetchPendingCloudJobs()
          } else if (payload.eventType === 'UPDATE') {
            const updated = payload.new as CloudPrintJob
            setJobs(prev =>
              prev.map(j => {
                if (j.cloud_job_id === updated.id) {
                  return {
                    ...j,
                    status: updated.status === 'printed' ? 'completed' : updated.status === 'rejected' ? 'canceled' : (updated.status as JobStatus),
                    message: `Status updated to ${updated.status}`,
                  }
                }
                return j
              })
            )
          }
        }
      )
      .subscribe(status => {
        setIsCloudConnected(status === 'SUBSCRIBED')
      })

    return () => {
      clearInterval(pollInterval)
      supabase.removeChannel(channel)
    }
  }, [shopId])

  const handleSaveShop = async () => {
    const trimmedShopName = shopName.trim()
    if (!trimmedShopName) {
      alert('Please enter your Shop Name.')
      return
    }
    let cleanId = shopId.trim().toLowerCase().replace(/[^a-z0-9_]/g, '_')
    if (!cleanId) {
      cleanId = trimmedShopName.toLowerCase().replace(/[^a-z0-9]/g, '_').replace(/_+/g, '_').replace(/^_|_$/g, '')
    }
    if (!cleanId) cleanId = 'shop_' + Math.floor(1000 + Math.random() * 9000)

    const cleanUrl = sanitizePublicUrl(publicAppUrl)
    setIsSavingShop(true)
    setShopSaveSuccess(false)
    try {
      localStorage.setItem('nearby_shop_unique_id', cleanId)
      localStorage.setItem('nearby_shop_name', trimmedShopName)
      localStorage.setItem('nearby_owner_name', ownerName.trim())
      localStorage.setItem('nearby_public_app_url', cleanUrl)
      localStorage.setItem('nearby_shop_configured', 'true')
      setShopId(cleanId)
      setShopName(trimmedShopName)
      setPublicAppUrl(cleanUrl)

      // Upsert to Supabase shops table
      const { error } = await supabase
        .from('shops')
        .upsert(
          {
            shop_unique_id: cleanId,
            shop_name: trimmedShopName,
            owner_name: ownerName.trim() || 'Store Counter',
          },
          { onConflict: 'shop_unique_id' }
        )

      if (error) throw error

      setShopSaveSuccess(true)
      setTimeout(() => setShopSaveSuccess(false), 3000)
      setShowShopModal(false)
    } catch (err: unknown) {
      console.error('Failed to save shop:', err)
      alert(`Could not save shop to Supabase: ${String(err)}`)
    } finally {
      setIsSavingShop(false)
    }
  }

  const handleFixFirewall = async () => {
    setIsFixingFirewall(true)
    setFirewallStatus('')
    try {
      const res = await invoke<string>('fix_windows_firewall')
      setFirewallStatus(res)
      const ok = await invoke<boolean>('check_firewall_status')
      setFirewallOk(ok)
    } catch (err: unknown) {
      setFirewallStatus(`Error: ${String(err)}`)
    } finally {
      setIsFixingFirewall(false)
    }
  }

  const serverIp = (() => {
    try {
      if (!serverUrl) return ''
      const match = serverUrl.match(/:\/\/([^:/]+)/)
      return match ? match[1] : ''
    } catch {
      return ''
    }
  })()

  const activeIp = (selectedIp && localIps.includes(selectedIp))
    ? selectedIp
    : (localIps.find(ip => ip !== '127.0.0.1') || serverIp || localIps[0] || '127.0.0.1')

  const localPortalUrl = useHostnameQr && hostnameUrl
    ? hostnameUrl
    : (activeIp !== '127.0.0.1' ? `http://${activeIp}:${activePort}/portal` : (serverUrl ? `${serverUrl.replace(/\/$/, '')}/portal` : `http://localhost:${activePort}/portal`))

  const cleanShop = (shopId || 'my_shop').trim().toLowerCase().replace(/[^a-z0-9_]/g, '_')
  const permanentCustomerUrl = `${sanitizePublicUrl(publicAppUrl)}/upload?shop=${cleanShop}`

  const handleCopyLink = () => {
    navigator.clipboard.writeText(permanentCustomerUrl)
    setCopiedLink(true)
    setTimeout(() => setCopiedLink(false), 2200)
  }

  const handleCopyLocalLink = () => {
    navigator.clipboard.writeText(localPortalUrl)
    setCopiedLocalLink(true)
    setTimeout(() => setCopiedLocalLink(false), 2200)
  }

  const handleCopyIpp = () => {
    navigator.clipboard.writeText(wirelessIppUrl)
    setCopiedIpp(true)
    setTimeout(() => setCopiedIpp(false), 2200)
  }

  const handlePrintLocalQrCard = () => {
    setPrintStandeeType('wifi')
    setTimeout(() => {
      window.print()
    }, 150)
  }

  const handlePrintPermanentQrCard = () => {
    setPrintStandeeType('cloud')
    setTimeout(() => {
      window.print()
    }, 150)
  }

  const maybeAutoPrint = (job: PrintJob) => {
    const isWeb = job.channel === 'web'
    const allowed = isWeb ? autoPrintWebRef.current : autoPrintWifiRef.current
    if (!allowed) return
    if (job.bytes <= 0 || job.status !== 'pending') return
    if (inFlight.current.has(job.id)) return
    const printer = getJobPrinter(job.color_mode)
    if (!printer) return
    inFlight.current.add(job.id)
    invoke('print_file', { jobId: Number(job.id), printer })
      .catch((e: unknown) => console.error('Auto print failed:', e))
      .finally(() => inFlight.current.delete(job.id))
  }

  const upsertJob = (job: PrintJob) => {
    setJobs(prev =>
      prev.some(j => j.id === job.id)
        ? prev.map(j => (j.id === job.id ? job : j))
        : [job, ...prev]
    )
    maybeAutoPrint(job)
  }

  // Listen for local IPP & Web Print events
  useEffect(() => {
    const unlisteners: UnlistenFn[] = []
    let cancelled = false

    const register = (event: string) => {
      listen<PrintJob>(event, e => {
        if (cancelled) return
        upsertJob(e.payload)
      })
        .then(fn => {
          if (cancelled) fn()
          else unlisteners.push(fn)
        })
        .catch(err => console.error(`Failed to listen to ${event}:`, err))
    }

    register('new-print-job')
    register('job_updated')

    return () => {
      cancelled = true
      unlisteners.forEach(fn => fn())
    }
  }, [])

  // Poll server status & printer list
  const refreshStatus = async () => {
    try {
      let running = false
      let url = ''
      let error = ''
      try {
        const bs = await invoke<{ broadcasting?: boolean; running?: boolean; url?: string; last_error?: string; error?: string }>('get_broadcast_status')
        running = bs.broadcasting ?? bs.running ?? false
        url = bs.url || ''
        error = bs.last_error || bs.error || ''
      } catch {
        const s = await invoke<{ running?: boolean; broadcasting?: boolean; url?: string; error?: string; last_error?: string }>('get_server_status')
        running = s.running ?? s.broadcasting ?? false
        url = s.url || ''
        error = s.error || s.last_error || ''
      }
      setIsBroadcasting(running)
      setServerUrl(url)
      setBroadcastError(error)
    } catch (e) {
      console.error('Failed to get server status:', e)
    }
  }

  const refreshPrinters = async () => {
    setIsFetchingPrinters(true)
    try {
      let list: PrinterInfo[] = []
      try {
        list = await invoke<PrinterInfo[]>('get_printers')
      } catch {
        list = await invoke<PrinterInfo[]>('get_available_printers')
      }
      setAvailablePrinters(list)

      const usable = list.filter(p => !p.isSelf)
      if (usable.length > 0) {
        const isVirtual = (name: string) => /pdf|onenote|fax|xps/i.test(name)
        const physical = usable.filter(p => !isVirtual(p.name))

        if (!selectedBwPrinter || !usable.some(p => p.name === selectedBwPrinter) || isVirtual(selectedBwPrinter)) {
          const hpLaserJet = usable.find(p => /laserjet/i.test(p.name))
          const defaultBw = hpLaserJet?.name
            || physical.find(p => p.online)?.name
            || physical[0]?.name
            || usable.find(p => p.online)?.name
            || usable[0].name
          setSelectedBwPrinter(defaultBw)
        }
        if (!selectedColorPrinter || !usable.some(p => p.name === selectedColorPrinter)) {
          const colorPrn = physical.find(p => /epson|canon|color|inktank/i.test(p.name))
          if (colorPrn) {
            setSelectedColorPrinter(colorPrn.name)
          }
        }
      }
    } catch (e) {
      console.error('Failed to load printers:', e)
    } finally {
      setIsFetchingPrinters(false)
    }
  }

  const refreshAll = async () => {
    setIsRefreshing(true)
    await Promise.all([refreshStatus(), refreshPrinters()])
    // Auto-restore local wireless IPP & mDNS broadcasting if stopped
    try {
      const net = await invoke<{ running?: boolean; port?: number }>('get_network_status').catch(() => null)
      if (net && !net.running) {
        await invoke('toggle_broadcast', { enable: true })
        await refreshStatus()
      }
    } catch {}
    setIsRefreshing(false)
  }

  useEffect(() => {
    refreshAll()
    const interval = setInterval(refreshStatus, 4000)
    return () => clearInterval(interval)
  }, [])

  const toggleBroadcast = async () => {
    try {
      const next = !isBroadcasting
      await invoke('toggle_broadcast', { enable: next })
      await refreshStatus()
    } catch (e: unknown) {
      console.error('Failed to toggle broadcast:', e)
      setBroadcastError(String(e))
    }
  }

  const toggleTheme = () => {
    setTheme(t => (t === 'light' ? 'dark' : 'light'))
  }

  const savePrinterName = async () => {
    if (!deviceName.trim()) return
    setIsSavingName(true)
    try {
      await invoke('update_printer_name', { name: deviceName.trim() })
    } catch (e) {
      console.error('Failed to update printer name:', e)
    } finally {
      setIsSavingName(false)
    }
  }

  const handleOpenFile = async (filePath?: string) => {
    if (!filePath) return
    try {
      await openPath(filePath)
    } catch (err) {
      console.error('Failed to open file:', err)
    }
  }

  // Accept and print a local job
  const handleAcceptLocalJob = async (job: PrintJob) => {
    const printer = getJobPrinter(job.color_mode)
    if (!printer) return
    inFlight.current.add(job.id)
    try {
      await invoke('print_file', { jobId: Number(job.id), printer })
    } catch (e) {
      console.error('Print failed', e)
    } finally {
      inFlight.current.delete(job.id)
    }
  }

  // Accept and print a Cloud job (Download -> Print -> Update Supabase -> Delete from temp_docs)
  const handleAcceptCloudJob = async (job: PrintJob) => {
    if (!job.file_url) return
    const targetPrinter = getJobPrinter(job.color_mode)
    if (!targetPrinter) {
      alert('Please select a printer in Dual Printer Assignment first.')
      return
    }

    if (inFlight.current.has(job.id)) return
    inFlight.current.add(job.id)

    // Update status locally to printing
    setJobs(prev =>
      prev.map(j => (j.id === job.id ? { ...j, status: 'printing', message: `Printing on ${targetPrinter}…` } : j))
    )

    try {
      // 1. Download file bytes from Supabase Storage
      const res = await fetch(job.file_url)
      if (!res.ok) throw new Error(`Download failed: ${res.statusText}`)
      const arrayBuffer = await res.arrayBuffer()
      const bytes = Array.from(new Uint8Array(arrayBuffer))

      // 2. Submit to local print queue
      const isPdf = job.name.toLowerCase().endsWith('.pdf') || (job.file_url && job.file_url.toLowerCase().includes('.pdf'))
      const isPng = job.name.toLowerCase().endsWith('.png') || (job.file_url && job.file_url.toLowerCase().includes('.png'))
      const contentType = isPdf ? 'application/pdf' : (isPng ? 'image/png' : 'image/jpeg')

      const localJobId = await invoke<number>('submit_cloud_job', {
        customer: job.customer || job.user,
        filename: job.name,
        data: bytes,
        contentType,
        copies: 1,
        colorMode: job.color_mode || 'bw',
      })

      // 3. Print the document
      await invoke('print_file', { jobId: localJobId, printer: targetPrinter })

      // 4. Update status in Supabase table
      if (job.cloud_job_id) {
        await supabase
          .from('print_jobs')
          .update({ status: 'printed' })
          .eq('id', job.cloud_job_id)

        // 5. Automatically delete file from temp_docs storage bucket!
        await deleteTempDoc(job.file_url)
      }

      setJobs(prev =>
        prev.map(j => (j.id === job.id ? { ...j, status: 'completed', message: `Printed on ${targetPrinter}` } : j))
      )
    } catch (err: unknown) {
      console.error('Cloud print execution failed:', err)
      setJobs(prev =>
        prev.map(j => (j.id === job.id ? { ...j, status: 'error', message: `Print failed: ${String(err)}` } : j))
      )
    } finally {
      inFlight.current.delete(job.id)
    }
  }

  // Reject a job
  const handleRejectJob = async (job: PrintJob) => {
    if (job.channel === 'cloud') {
      if (job.cloud_job_id) {
        try {
          await supabase
            .from('print_jobs')
            .update({ status: 'rejected' })
            .eq('id', job.cloud_job_id)

          if (job.file_url) {
            await deleteTempDoc(job.file_url)
          }
        } catch (e) {
          console.warn('Reject error:', e)
        }
      }
      setJobs(prev => prev.map(j => (j.id === job.id ? { ...j, status: 'canceled', message: 'Rejected' } : j)))
    } else {
      invoke('cancel_job', { jobId: Number(job.id) }).catch(e => console.error('Cancel failed', e))
    }
  }

  const selectRelayHost = async (chosen: string) => {
    if (isSavingRelay) return
    setIsSavingRelay(true)
    setRelayResult(null)
    try {
      const next = await invoke<RelayStatus>('set_relay_host', { host: chosen })
      setRelayStatus(next)
      setRelayHost(next.host)
      setRelayWarning(next.warning)
      setRelayResult({
        ok: true,
        message: chosen ? `Relay forwarding enabled to ${chosen}` : 'Relay forwarding disabled.',
      })
    } catch (err: unknown) {
      setRelayResult({ ok: false, message: String(err) })
    } finally {
      setIsSavingRelay(false)
    }
  }

  const usablePrinters = availablePrinters.filter(p => !p.isSelf)
  const pendingJobs = jobs.filter(j => j.status === 'pending')
  const pendingCloudJobs = pendingJobs.filter(j => j.channel === 'cloud')
  const pendingLocalJobs = pendingJobs.filter(j => j.channel !== 'cloud')

  const filteredJobs = jobs.filter(j => {
    if (queueFilter === 'cloud') return j.channel === 'cloud'
    if (queueFilter === 'wifi') return j.channel !== 'cloud'
    return true
  })

  const wirelessIppUrl = `ipp://${activeIp}:${activePort}/ipp/print`

  return (
    <>
    <div id="app-main-ui" className="min-h-screen bg-background text-foreground flex flex-col font-sans transition-colors duration-200">
      {/* Header */}
      <header className="flex items-center justify-between px-6 py-3.5 border-b border-border bg-card/60 backdrop-blur-md sticky top-0 z-30">
        <div className="flex items-center gap-3">
          <div className="p-2 bg-primary/10 rounded-xl text-primary">
            <Printer className="w-5 h-5" />
          </div>
          <div className="flex flex-col">
            <div className="flex items-center gap-2">
              <h1 className="text-lg font-bold tracking-tight">NearbyPrint</h1>
              <span className="text-[10px] font-bold px-1.5 py-0.5 rounded-md bg-indigo-500/15 text-indigo-500 dark:text-indigo-400 border border-indigo-500/30 uppercase tracking-wider">
                BETA
              </span>
              <span className="text-[11px] font-semibold px-2 py-0.5 rounded-full bg-emerald-500/15 text-emerald-600 dark:text-emerald-400 flex items-center gap-1">
                <span className="w-1.5 h-1.5 rounded-full bg-emerald-500" />
                Cloud Active
              </span>
            </div>
            {shopName ? (
              <button
                type="button"
                onClick={() => setShowShopModal(true)}
                className="text-xs text-muted-foreground hover:text-foreground truncate max-w-[340px] text-left flex items-center gap-1 group mt-0.5"
                title="Click to edit shop details"
              >
                <span>{shopName} {ownerName ? `(${ownerName})` : ''} · {deviceName || 'Smart Print Counter'}</span>
                <span className="text-[10px] text-indigo-500 opacity-60 group-hover:opacity-100">✏️</span>
              </button>
            ) : (
              <button
                type="button"
                onClick={() => setShowShopModal(true)}
                className="text-xs font-semibold text-amber-500 hover:text-amber-400 flex items-center gap-1 mt-0.5 animate-pulse text-left"
              >
                ⚠️ Click here to set your Shop Name &amp; QR
              </button>
            )}
          </div>
        </div>

        <div className="flex items-center gap-1.5">
          <Button
            variant="ghost"
            size="icon"
            onClick={refreshAll}
            disabled={isRefreshing}
            title="Refresh printers"
            aria-label="Refresh printers"
            className="h-9 w-9"
          >
            <RefreshCw className={`w-4 h-4 ${isRefreshing ? 'animate-spin' : ''}`} />
          </Button>
          <Button variant="ghost" size="icon" onClick={toggleTheme} className="h-9 w-9" title="Toggle Theme">
            {theme === 'light' ? <Moon className="w-4 h-4" /> : <Sun className="w-4 h-4" />}
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={() => setShowSettings(true)}
            className="gap-2 ml-1 h-9 font-medium shadow-sm"
          >
            <Settings className="w-4 h-4 text-primary" />
            <span>Settings</span>
          </Button>
        </div>
      </header>

      <main className="flex-1 max-w-3xl mx-auto w-full flex flex-col gap-5 px-5 py-5">
        {/* PERMANENT CLOUD SAAS COUNTER & QR CODE */}
        <section className="bg-gradient-to-br from-card via-card to-indigo-500/5 border border-border rounded-2xl p-5 shadow-sm flex flex-col gap-4">
            <div className="flex flex-col sm:flex-row items-start sm:items-center justify-between gap-3">
              <div className="flex flex-col gap-0.5">
                <div className="flex items-center gap-2">
                  <Cloud className="w-4 h-4 text-indigo-500" />
                  <h2 className="text-base font-bold">
                    Permanent Cloud Counter (Any Internet / 4G / 5G / Broadband)
                  </h2>
                </div>
                <p className="text-xs text-muted-foreground">
                  Customers can send prints over 4G/5G/Broadband from anywhere directly to this counter.
                </p>
              </div>
              <div className="flex items-center gap-2 flex-wrap">
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => setShowShopModal(true)}
                  className="gap-1.5 text-xs h-8"
                >
                  <Store className="w-3.5 h-3.5 text-indigo-500" />
                  Edit Shop ID
                </Button>
                <Button
                  size="sm"
                  onClick={handlePrintPermanentQrCard}
                  className="gap-1.5 text-xs h-8 bg-indigo-600 hover:bg-indigo-700 text-white shadow-sm font-semibold"
                >
                  <Printer className="w-3.5 h-3.5" />
                  Print Cloud Standee
                </Button>
              </div>
            </div>

            <div className="flex flex-col sm:flex-row items-center gap-5 bg-muted/40 border border-border rounded-xl p-4">
              {/* Permanent QR Code */}
              <div className="flex flex-col items-center gap-1.5 shrink-0">
                <div
                  className="p-2 bg-white rounded-xl shadow-sm border border-slate-200 cursor-pointer hover:scale-105 transition-transform"
                  onClick={() => {
                    setModalQrTab('cloud')
                    setShowQrModal(true)
                  }}
                  title="Click to zoom Permanent QR"
                >
                  <QRCodeSVG
                    id="permanent-counter-qr"
                    value={permanentCustomerUrl}
                    size={110}
                    level="M"
                    includeMargin={false}
                  />
                </div>
                <span className="text-[10px] text-muted-foreground font-semibold flex items-center gap-1">
                  <QrCode className="w-3 h-3 text-indigo-500" /> Cloud Internet QR
                </span>
              </div>

              {/* Permanent Shop Info & Link */}
              <div className="flex flex-col gap-3 min-w-0 flex-1 w-full">
                {/* Shop Badge & ID */}
                <div className="flex items-center justify-between flex-wrap gap-2">
                  <div className="flex items-center gap-2">
                    <span className="font-extrabold text-sm text-foreground">{shopName}</span>
                    <span className="font-mono text-[11px] bg-indigo-500/15 text-indigo-600 dark:text-indigo-400 border border-indigo-500/30 px-2 py-0.5 rounded-full font-semibold">
                      ID: {shopId}
                    </span>
                  </div>
                  {supabaseError ? (
                    <span className="text-[11px] text-red-500 font-bold flex items-center gap-1 bg-red-500/10 px-2 py-0.5 rounded border border-red-500/20">
                      <AlertTriangle className="w-3 h-3 text-red-500 shrink-0" />
                      Supabase Error: {supabaseError}
                    </span>
                  ) : (
                    <span className="text-[11px] text-emerald-600 dark:text-emerald-400 font-semibold flex items-center gap-1">
                      <CheckCircle2 className="w-3 h-3" /> Supabase Realtime Active
                    </span>
                  )}
                </div>

                {/* Shareable Link Input with Copy */}
                <div className="flex flex-col gap-1">
                  <span className="text-[11px] text-muted-foreground font-semibold uppercase tracking-wider flex items-center gap-1.5">
                    <Globe className="w-3 h-3 text-indigo-500" /> Customer Upload Link (Internet)
                  </span>
                  <div className="flex items-center gap-2">
                    <span className="font-mono text-xs bg-background border border-border px-2.5 py-1.5 rounded-lg text-indigo-600 dark:text-indigo-400 font-semibold select-all truncate flex-1">
                      {permanentCustomerUrl}
                    </span>
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={handleCopyLink}
                      className="h-8 px-2.5 text-xs gap-1 shrink-0"
                      title="Copy Permanent Link"
                    >
                      {copiedLink ? <Check className="w-3.5 h-3.5 text-emerald-500" /> : <Copy className="w-3.5 h-3.5" />}
                      <span>{copiedLink ? 'Copied!' : 'Copy'}</span>
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => window.open(permanentCustomerUrl, '_blank')}
                      className="h-8 px-2 text-xs gap-1 shrink-0 text-muted-foreground hover:text-foreground"
                      title="Preview Customer Upload Portal"
                    >
                      <ExternalLink className="w-3.5 h-3.5" />
                    </Button>
                  </div>
                </div>

                {/* Cloud Auto-Print Control */}
                <div className="pt-1 border-t border-border/60">
                  <div className="flex items-center justify-between p-2 rounded-lg bg-background/60 border border-border/60">
                    <div className="flex items-center gap-2">
                      <Cloud className="w-3.5 h-3.5 text-indigo-500" />
                      <span className="text-xs font-semibold">Auto-Print: Cloud Web Queue</span>
                    </div>
                    <Switch
                      checked={autoPrintCloud}
                      onCheckedChange={setAutoPrintCloud}
                    />
                  </div>
                </div>
              </div>
            </div>
          </section>

        {/* SECTION 2: DUAL PRINTER ASSIGNMENT (B&W vs COLOR) */}
        <section className="bg-card border border-border rounded-2xl p-5 shadow-sm flex flex-col gap-3">
          <div className="flex items-center justify-between">
            <div className="flex flex-col gap-0.5">
              <h2 className="text-base font-semibold flex items-center gap-2">
                <Printer className="w-4 h-4 text-primary" />
                Dual Printer Assignment
              </h2>
              <p className="text-xs text-muted-foreground">
                Black &amp; White and Color jobs automatically route to their designated printer.
              </p>
            </div>
          </div>

          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3.5 mt-1">
            {/* Black & White Printer Card */}
            <div className="p-3.5 bg-muted/40 border border-border rounded-xl flex flex-col gap-2">
              <div className="flex items-center justify-between">
                <span className="text-xs font-semibold uppercase tracking-wider text-muted-foreground flex items-center gap-1.5">
                  <span className="w-2.5 h-2.5 rounded-full bg-slate-800 dark:bg-slate-300" />
                  B&amp;W Printer (LaserJet)
                </span>
                <span className="text-[11px] text-muted-foreground">Standard Xerox &amp; Docs</span>
              </div>
              <select
                value={selectedBwPrinter}
                onChange={e => setSelectedBwPrinter(e.target.value)}
                className="w-full text-sm font-medium bg-background border border-border rounded-lg p-2.5 outline-none text-foreground cursor-pointer focus:ring-2 focus:ring-primary/20"
              >
                {usablePrinters.length === 0 && <option value="">No printers found</option>}
                {usablePrinters.map(p => (
                  <option key={p.name} value={p.name}>
                    {p.name} {p.online ? '(Online)' : '(Offline)'}
                  </option>
                ))}
              </select>
            </div>

            {/* Color Printer Card */}
            <div className="p-3.5 bg-muted/40 border border-border rounded-xl flex flex-col gap-2">
              <div className="flex items-center justify-between">
                <span className="text-xs font-semibold uppercase tracking-wider text-muted-foreground flex items-center gap-1.5">
                  <span className="w-2.5 h-2.5 rounded-full bg-gradient-to-r from-pink-500 to-indigo-500" />
                  Color Printer (InkTank)
                </span>
                <span className="text-[11px] text-muted-foreground">Color Photos &amp; Forms</span>
              </div>
              <select
                value={selectedColorPrinter}
                onChange={e => setSelectedColorPrinter(e.target.value)}
                className="w-full text-sm font-medium bg-background border border-border rounded-lg p-2.5 outline-none text-foreground cursor-pointer focus:ring-2 focus:ring-primary/20"
              >
                <option value="">Same as B&amp;W Printer</option>
                {usablePrinters.map(p => (
                  <option key={p.name} value={p.name}>
                    {p.name} {p.online ? '(Online)' : '(Offline)'}
                  </option>
                ))}
              </select>
            </div>
          </div>
        </section>

        {/* SECTION 3: DUAL-CHANNEL PRINT QUEUE */}
        <section className="flex flex-col gap-3 mb-6">
          <div className="flex flex-col sm:flex-row items-start sm:items-center justify-between gap-2 ml-1">
            <div className="flex items-center gap-2">
              <h3 className="text-sm font-bold uppercase tracking-wider flex items-center gap-2">
                Print Queue
              </h3>
              {pendingJobs.length > 0 && (
                <span className="bg-amber-500/20 text-amber-600 dark:text-amber-400 text-xs px-2.5 py-0.5 rounded-full font-bold animate-pulse">
                  {pendingJobs.length} Waiting
                </span>
              )}
            </div>

            {/* Filter Tabs */}
            <div className="flex items-center gap-1 bg-muted/60 p-1 rounded-xl border border-border text-xs">
              <button
                type="button"
                onClick={() => setQueueFilter('all')}
                className={`px-3 py-1 rounded-lg font-semibold transition-all ${
                  queueFilter === 'all'
                    ? 'bg-background text-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground'
                }`}
              >
                All ({jobs.length})
              </button>
              <button
                type="button"
                onClick={() => setQueueFilter('cloud')}
                className={`px-3 py-1 rounded-lg font-semibold flex items-center gap-1.5 transition-all ${
                  queueFilter === 'cloud'
                    ? 'bg-indigo-600 text-white shadow-sm'
                    : 'text-muted-foreground hover:text-foreground'
                }`}
              >
                <Cloud className="w-3 h-3" />
                Cloud ({jobs.filter(j => j.channel === 'cloud').length})
                {pendingCloudJobs.length > 0 && (
                  <span className="w-2 h-2 rounded-full bg-amber-400" />
                )}
              </button>
              {jobs.filter(j => j.channel !== 'cloud').length > 0 && (
                <button
                  type="button"
                  onClick={() => setQueueFilter('wifi')}
                  className={`px-3 py-1 rounded-lg font-semibold flex items-center gap-1.5 transition-all ${
                    queueFilter === 'wifi'
                      ? 'bg-sky-600 text-white shadow-sm'
                      : 'text-muted-foreground hover:text-foreground'
                  }`}
                >
                  <Wifi className="w-3 h-3" />
                  Wi-Fi ({jobs.filter(j => j.channel !== 'cloud').length})
                </button>
              )}
            </div>
          </div>

          {/* Cloud Polling Status & Supabase Error Notification */}
          <div className="flex flex-wrap items-center justify-between gap-2 bg-card/60 border border-border px-3 py-2 rounded-xl text-xs">
            <div className="flex items-center gap-2">
              <span className="w-2 h-2 rounded-full bg-emerald-500 animate-pulse" />
              <span className="text-muted-foreground font-medium">Cloud Polling (every 3s):</span>
              <span className="font-mono bg-indigo-500/15 text-indigo-600 dark:text-indigo-400 border border-indigo-500/30 px-2 py-0.5 rounded-md font-semibold">
                shop_unique_id = {shopId || '(Not Set)'}
              </span>
              <button
                type="button"
                onClick={() => setShowShopModal(true)}
                className="text-indigo-500 hover:text-indigo-600 dark:hover:text-indigo-400 underline font-semibold ml-1 cursor-pointer"
              >
                Change Shop ID
              </button>
            </div>
            {supabaseError ? (
              <span className="text-red-500 dark:text-red-400 font-bold flex items-center gap-1.5 bg-red-500/10 px-2.5 py-1 rounded-md border border-red-500/20">
                <AlertTriangle className="w-3.5 h-3.5 shrink-0" />
                <span>Error: {supabaseError}</span>
              </span>
            ) : (
              <span className="text-muted-foreground text-[11px]">
                Query: <span className="font-mono text-foreground font-semibold">status = 'pending'</span>
              </span>
            )}
          </div>

          {filteredJobs.length === 0 ? (
            <div className="bg-card/50 border border-border border-dashed rounded-xl p-8 flex flex-col items-center justify-center text-muted-foreground text-center gap-2">
              <FileText className="w-8 h-8 opacity-40" />
              <p className="text-sm font-medium">No print jobs in this view</p>
              <p className="text-xs opacity-75">
                Customer internet uploads and Wi-Fi prints will appear here in realtime.
              </p>
            </div>
          ) : (
            <div className="flex flex-col gap-3">
              {filteredJobs.map(job => {
                const targetPrinter = getJobPrinter(job.color_mode)
                return (
                  <div
                    key={job.id}
                    className="bg-card border border-border rounded-xl p-4 flex flex-col gap-3 shadow-sm hover:border-border/80 transition-all"
                  >
                    <div className="flex items-start justify-between gap-4">
                      <div className="flex items-center gap-3.5 min-w-0">
                        {job.file_path ? (
                          <button
                            type="button"
                            onClick={() => handleOpenFile(job.file_path)}
                            title="Open / Preview File"
                            className="p-3 bg-muted rounded-xl shrink-0 cursor-pointer transition-all hover:bg-muted/80 hover:scale-105 group"
                          >
                            <FileText className="w-6 h-6 text-foreground transition-opacity group-hover:opacity-75" />
                          </button>
                        ) : job.file_url ? (
                          <button
                            type="button"
                            onClick={() => window.open(job.file_url, '_blank')}
                            title="View Cloud Document"
                            className="p-3 bg-indigo-500/10 text-indigo-500 rounded-xl shrink-0 cursor-pointer transition-all hover:scale-105"
                          >
                            <FileText className="w-6 h-6" />
                          </button>
                        ) : (
                          <div className="p-3 bg-muted rounded-xl shrink-0">
                            <FileText className="w-6 h-6 text-foreground opacity-40" />
                          </div>
                        )}

                        <div className="flex flex-col min-w-0">
                          <div className="flex items-center gap-1.5 flex-wrap mb-1">
                            {job.channel === 'cloud' ? (
                              <span className="inline-flex items-center gap-1 text-[11px] font-semibold text-indigo-600 dark:text-indigo-400 bg-indigo-500/15 border border-indigo-500/30 px-2 py-0.5 rounded-full">
                                <Cloud className="w-3 h-3" /> Cloud Internet
                              </span>
                            ) : job.channel === 'web' ? (
                              <span className="inline-flex items-center gap-1 text-[11px] font-semibold text-purple-600 dark:text-purple-400 bg-purple-500/10 px-2 py-0.5 rounded-full">
                                <Globe className="w-3 h-3" /> Local Portal
                              </span>
                            ) : (
                              <span className="inline-flex items-center gap-1 text-[11px] font-semibold text-sky-600 dark:text-sky-400 bg-sky-500/10 px-2 py-0.5 rounded-full">
                                <Wifi className="w-3 h-3" /> Wi-Fi Direct
                              </span>
                            )}

                            {job.is_card && (
                              <span className="inline-flex items-center gap-1 text-[11px] font-semibold text-emerald-600 dark:text-emerald-400 bg-emerald-500/10 px-2 py-0.5 rounded-full">
                                🪪 ID Card CR80
                              </span>
                            )}

                            <span className="text-[11px] font-medium text-muted-foreground bg-muted px-1.5 py-0.5 rounded">
                              {job.color_mode === 'color' ? '🎨 Color' : '⚫ B&W'}
                            </span>

                            {targetPrinter && (
                              <span
                                className="text-[11px] font-mono text-muted-foreground bg-muted/60 px-1.5 py-0.5 rounded truncate max-w-[160px]"
                                title={`Routes to ${targetPrinter}`}
                              >
                                → {targetPrinter}
                              </span>
                            )}
                          </div>

                          <span className="font-semibold text-base truncate flex items-center gap-2">
                            {job.customer ? (
                              <span className="text-primary font-bold">{job.customer}:</span>
                            ) : null}
                            <span className="truncate">{job.name}</span>
                          </span>

                          <span className="text-xs text-muted-foreground truncate">
                            {job.device}
                            {job.bytes > 0 && ` · ${Math.max(1, Math.round(job.bytes / 1024))} KB`}
                          </span>

                          {job.message && (
                            <span
                              className={`text-xs mt-1 ${
                                job.status === 'error'
                                  ? 'text-red-500 font-medium'
                                  : 'text-muted-foreground'
                              }`}
                            >
                              {job.message}
                            </span>
                          )}
                        </div>
                      </div>

                      <span
                        className={`text-xs font-semibold px-2.5 py-1 rounded-full whitespace-nowrap ${
                          statusStyle[job.status]
                        }`}
                      >
                        {statusLabel[job.status]}
                      </span>
                    </div>

                    {/* Action Bar for Pending Jobs */}
                    {job.status === 'pending' && (
                      <div className="flex items-center justify-end gap-2 border-t border-border/50 pt-2.5 mt-1">
                        {job.file_url && (
                          <Button
                            variant="outline"
                            size="sm"
                            onClick={() => window.open(job.file_url, '_blank')}
                            className="gap-1.5 h-8 text-xs"
                          >
                            <ExternalLink className="w-3.5 h-3.5" /> View Doc
                          </Button>
                        )}
                        {job.file_path && (
                          <Button
                            variant="outline"
                            size="sm"
                            onClick={() => handleOpenFile(job.file_path)}
                            className="gap-1.5 h-8 text-xs"
                          >
                            <FolderOpen className="w-3.5 h-3.5" /> Preview
                          </Button>
                        )}

                        <Button
                          variant="outline"
                          size="sm"
                          onClick={() => handleRejectJob(job)}
                          className="text-red-500 hover:text-red-600 hover:bg-red-500/10 border-red-500/20 h-8 text-xs"
                        >
                          <X className="w-3.5 h-3.5 mr-1" /> Reject
                        </Button>

                        <Button
                          size="sm"
                          onClick={() =>
                            job.channel === 'cloud'
                              ? handleAcceptCloudJob(job)
                              : handleAcceptLocalJob(job)
                          }
                          className="h-8 text-xs bg-emerald-600 hover:bg-emerald-700 text-white gap-1 font-semibold"
                        >
                          <Check className="w-3.5 h-3.5 mr-1" /> Accept &amp; Print
                        </Button>
                      </div>
                    )}
                  </div>
                )
              })}
            </div>
          )}
        </section>
      </main>

      {/* Edit Shop ID / SaaS Modal */}
      {showShopModal && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm p-4"
          onClick={() => setShowShopModal(false)}
        >
          <div
            className="bg-card border border-border rounded-2xl shadow-2xl w-full max-w-md overflow-hidden flex flex-col p-5 gap-4"
            onClick={e => e.stopPropagation()}
          >
            <div className="flex items-center justify-between pb-2 border-b border-border">
              <div className="flex items-center gap-2">
                <Store className="w-5 h-5 text-indigo-500" />
                <h3 className="font-bold text-base">Permanent Shop Configuration</h3>
              </div>
              <Button variant="ghost" size="icon" onClick={() => setShowShopModal(false)} className="h-8 w-8">
                <X className="w-4 h-4" />
              </Button>
            </div>

            <div className="flex flex-col gap-3.5 text-xs">
              <p className="text-[11px] text-muted-foreground leading-relaxed">
                Enter your shop details below. A permanent QR code will be generated for your counter.
              </p>

              <div>
                <label className="font-semibold block mb-1">
                  Shop Display Name <span className="text-red-500">*</span>
                </label>
                <Input
                  value={shopName}
                  onChange={e => {
                    const val = e.target.value
                    setShopName(val)
                    // Auto-slugify if shopId is empty or was auto-generated
                    if (!shopId || shopId === shopName.toLowerCase().replace(/[^a-z0-9]/g, '_')) {
                      setShopId(val.toLowerCase().replace(/[^a-z0-9]/g, '_').replace(/_+/g, '_').replace(/^_|_$/g, ''))
                    }
                  }}
                  placeholder="e.g. Kunal Printing &amp; Xerox"
                  className="text-xs"
                  required
                />
              </div>

              <div>
                <label className="font-semibold block mb-1">
                  Permanent Shop Unique ID (URL Slug) <span className="text-red-500">*</span>
                </label>
                <Input
                  value={shopId}
                  onChange={e => setShopId(e.target.value.toLowerCase().replace(/[^a-z0-9_]/g, '_'))}
                  placeholder="e.g. kunal_printing"
                  className="font-mono text-xs"
                  required
                />
                <span className="text-[11px] text-muted-foreground mt-0.5 block">
                  Lowercase alphanumeric + underscore only. QR Link: <span className="font-mono text-indigo-500">{publicAppUrl}/upload?shop={shopId || 'your_shop'}</span>
                </span>
              </div>

              <div>
                <label className="font-semibold block mb-1">Owner / Counter Name</label>
                <Input
                  value={ownerName}
                  onChange={e => setOwnerName(e.target.value)}
                  placeholder="e.g. Kunal Kumar"
                  className="text-xs"
                />
              </div>

              <div>
                <label className="font-semibold block mb-1">Public Web App Domain (Vercel Host)</label>
                <Input
                  value={publicAppUrl}
                  onChange={e => setPublicAppUrl(e.target.value)}
                  placeholder="https://nearbyprint-web.vercel.app"
                  className="font-mono text-xs"
                />
                <span className="text-[11px] text-muted-foreground mt-0.5 block">
                  Your live Vercel deployment URL (defaults to https://nearbyprint-web.vercel.app).
                </span>
              </div>
            </div>

            <div className="flex items-center justify-end gap-2 pt-2 border-t border-border">
              <Button variant="outline" size="sm" onClick={() => setShowShopModal(false)}>
                Cancel
              </Button>
              <Button
                size="sm"
                onClick={handleSaveShop}
                disabled={isSavingShop}
                className="bg-indigo-600 hover:bg-indigo-700 text-white font-semibold gap-1.5"
              >
                {isSavingShop ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Check className="w-3.5 h-3.5" />}
                Save &amp; Sync to Cloud
              </Button>
            </div>
          </div>
        </div>
      )}

      {/* Enlarged QR Modal */}
      {showQrModal && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-sm p-4"
          onClick={() => setShowQrModal(false)}
        >
          <div
            className="bg-card border border-border rounded-2xl shadow-2xl p-6 flex flex-col items-center gap-4 max-w-sm w-full text-center"
            onClick={e => e.stopPropagation()}
          >
            <div className="flex items-center justify-between w-full">
              <span className="font-bold text-sm text-foreground">{shopName}</span>
              <Button variant="ghost" size="icon" onClick={() => setShowQrModal(false)} className="h-8 w-8">
                <X className="w-4 h-4" />
              </Button>
            </div>

            {/* QR Image */}
            <div className="p-3 bg-white rounded-2xl shadow-md border border-slate-200">
              <QRCodeSVG
                id="modal-cloud-qr"
                value={permanentCustomerUrl}
                size={220}
                level="M"
                includeMargin={false}
              />
            </div>

            <div className="flex flex-col gap-1 w-full text-center">
              <span className="text-[11px] font-semibold text-muted-foreground">
                ☁️ Permanent Customer Self-Service QR (Any Internet / 4G / 5G)
              </span>
              <div className="font-mono text-xs px-3 py-1.5 rounded-lg select-all break-all text-indigo-600 bg-indigo-500/10">
                {permanentCustomerUrl}
              </div>
            </div>

            <div className="flex gap-2 w-full pt-1">
              <Button
                variant="outline"
                size="sm"
                onClick={handleCopyLink}
                className="flex-1 text-xs"
              >
                {copiedLink ? 'Copied! ✓' : 'Copy Cloud Link'}
              </Button>
              <Button
                size="sm"
                onClick={handlePrintPermanentQrCard}
                className="flex-1 text-xs text-white bg-indigo-600 hover:bg-indigo-700"
              >
                Print Standee
              </Button>
            </div>
          </div>
        </div>
      )}

      {/* Settings Modal (Organized & Clean) */}
      {showSettings && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm p-4" onClick={() => setShowSettings(false)}>
          <div className="bg-card border border-border rounded-2xl shadow-2xl w-full max-w-xl overflow-hidden flex flex-col max-h-[88vh]" onClick={e => e.stopPropagation()}>
            <div className="flex items-center justify-between p-4 border-b border-border">
              <div className="flex items-center gap-2">
                <Settings className="w-5 h-5 text-primary" />
                <h2 className="text-base font-semibold">Settings &amp; Configuration</h2>
              </div>
              <Button variant="ghost" size="icon" onClick={() => setShowSettings(false)} className="h-8 w-8">
                <X className="w-4 h-4" />
              </Button>
            </div>

            {/* Settings Tabs */}
            <div className="flex border-b border-border bg-muted/40 px-3 pt-2 gap-1 overflow-x-auto">
              <button
                type="button"
                onClick={() => setSettingsTab('cloud')}
                className={`px-3 py-2 text-xs font-semibold rounded-t-lg transition-colors border-b-2 flex items-center gap-1.5 ${settingsTab === 'cloud' ? 'border-indigo-500 text-indigo-500 bg-card' : 'border-transparent text-muted-foreground hover:text-foreground'}`}
              >
                <Cloud className="w-3.5 h-3.5" /> Cloud SaaS &amp; Shop ID
              </button>
              <button
                type="button"
                onClick={() => setSettingsTab('network')}
                className={`px-3 py-2 text-xs font-semibold rounded-t-lg transition-colors border-b-2 flex items-center gap-1.5 ${settingsTab === 'network' ? 'border-primary text-primary bg-card' : 'border-transparent text-muted-foreground hover:text-foreground'}`}
              >
                <Network className="w-3.5 h-3.5" /> Wi-Fi LAN
              </button>
              <button
                type="button"
                onClick={() => setSettingsTab('firewall')}
                className={`px-3 py-2 text-xs font-semibold rounded-t-lg transition-colors border-b-2 flex items-center gap-1.5 ${settingsTab === 'firewall' ? 'border-primary text-primary bg-card' : 'border-transparent text-muted-foreground hover:text-foreground'}`}
              >
                <ShieldCheck className="w-3.5 h-3.5" /> Firewall
              </button>
              <button
                type="button"
                onClick={() => setSettingsTab('printers')}
                className={`px-3 py-2 text-xs font-semibold rounded-t-lg transition-colors border-b-2 flex items-center gap-1.5 ${settingsTab === 'printers' ? 'border-primary text-primary bg-card' : 'border-transparent text-muted-foreground hover:text-foreground'}`}
              >
                <Printer className="w-3.5 h-3.5" /> Printers
              </button>
            </div>

            <div className="p-5 flex-1 overflow-y-auto flex flex-col gap-4 text-xs">
              {/* Tab: Cloud SaaS */}
              {settingsTab === 'cloud' && (
                <div className="flex flex-col gap-4">
                  <div className="p-3.5 bg-indigo-500/10 border border-indigo-500/20 rounded-xl flex items-center justify-between">
                    <div>
                      <span className="font-bold text-indigo-500 text-xs block">Supabase SaaS Platform Status</span>
                      <span className="text-[11px] text-muted-foreground">URL: https://xxabrfeoskaiwhldqhyc.supabase.co</span>
                    </div>
                    <span className="text-xs font-bold text-emerald-500 bg-emerald-500/15 px-2 py-0.5 rounded-full">
                      Connected 🟢
                    </span>
                  </div>

                  <div className="flex flex-col gap-1.5">
                    <label className="font-semibold text-foreground text-xs">Permanent Shop Unique ID</label>
                    <Input
                      value={shopId}
                      onChange={e => {
                        const val = e.target.value.toLowerCase().replace(/[^a-z0-9_]/g, '_')
                        setShopId(val)
                        localStorage.setItem('nearby_shop_unique_id', val)
                      }}
                      className="font-mono text-xs"
                      placeholder="e.g. kunal_print"
                    />
                    <span className="text-[11px] text-muted-foreground">
                      Must match the 'shop_unique_id' in Supabase print_jobs table (e.g. <span className="font-mono text-indigo-500 font-semibold">kunal_print</span>).
                    </span>
                  </div>

                  <div className="flex flex-col gap-1.5">
                    <label className="font-semibold text-foreground text-xs">Shop Name</label>
                    <Input
                      value={shopName}
                      onChange={e => {
                        const val = e.target.value
                        setShopName(val)
                        localStorage.setItem('nearby_shop_name', val)
                      }}
                      className="text-xs"
                    />
                  </div>

                  <div className="flex flex-col gap-1.5">
                    <label className="font-semibold text-foreground text-xs">Public Customer Portal Domain</label>
                    <Input
                      value={publicAppUrl}
                      onChange={e => {
                        const val = e.target.value
                        setPublicAppUrl(val)
                        localStorage.setItem('nearby_public_app_url', val)
                      }}
                      className="font-mono text-xs"
                    />
                  </div>

                  <Button
                    onClick={handleSaveShop}
                    disabled={isSavingShop}
                    className="bg-indigo-600 hover:bg-indigo-700 text-white font-semibold text-xs mt-1"
                  >
                    {isSavingShop ? 'Saving…' : 'Save & Sync Shop Info'}
                  </Button>
                </div>
              )}

              {/* Tab: Wi-Fi LAN */}
              {settingsTab === 'network' && (
                <div className="flex flex-col gap-4">
                  {/* 1. Wireless Connection (IPP Address) */}
                  <div className="flex flex-col gap-1.5">
                    <label className="text-[11px] text-muted-foreground font-semibold uppercase tracking-wider flex items-center gap-1.5">
                      <Wifi className="w-3.5 h-3.5 text-sky-500" /> Wireless Connection (IPP Address)
                    </label>
                    <div className="flex items-center gap-2">
                      <span className="font-mono text-xs bg-background border border-border px-2.5 py-2 rounded-lg text-foreground font-semibold select-all truncate flex-1">
                        {wirelessIppUrl}
                      </span>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={handleCopyIpp}
                        className="h-9 px-3 text-xs gap-1.5 shrink-0"
                      >
                        {copiedIpp ? <Check className="w-3.5 h-3.5 text-emerald-500" /> : <Copy className="w-3.5 h-3.5" />}
                        <span>{copiedIpp ? 'Copied!' : 'Copy IPP'}</span>
                      </Button>
                    </div>
                    <span className="text-[11px] text-muted-foreground">
                      Use this IPP address to print directly from Android / iPhone / Mac on the same Wi-Fi router.
                    </span>
                  </div>

                  {/* 2. Customer Web Portal URL (Shop Wi-Fi) */}
                  <div className="flex flex-col gap-1.5">
                    <label className="text-[11px] text-muted-foreground font-semibold uppercase tracking-wider flex items-center gap-1.5">
                      <Globe className="w-3.5 h-3.5 text-purple-500" /> Local Customer Web Portal URL (Shop Wi-Fi)
                    </label>
                    <div className="flex items-center gap-2">
                      <span className="font-mono text-xs bg-background border border-border px-2.5 py-2 rounded-lg text-primary font-semibold select-all truncate flex-1">
                        {localPortalUrl}
                      </span>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={handleCopyLocalLink}
                        className="h-9 px-3 text-xs gap-1.5 shrink-0"
                      >
                        {copiedLocalLink ? <Check className="w-3.5 h-3.5 text-emerald-500" /> : <Copy className="w-3.5 h-3.5" />}
                        <span>{copiedLocalLink ? 'Copied!' : 'Copy Link'}</span>
                      </Button>
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => window.open(localPortalUrl, '_blank')}
                        className="h-9 px-2 text-xs gap-1 shrink-0 text-muted-foreground hover:text-foreground"
                        title="Open in Browser"
                      >
                        <ExternalLink className="w-3.5 h-3.5" />
                      </Button>
                    </div>
                  </div>

                  {/* 3. Active Network Adapter IP */}
                  <div className="flex flex-col gap-1.5">
                    <label className="font-semibold text-foreground text-xs">Active Network Adapter IP</label>
                    <select
                      value={activeIp}
                      onChange={e => {
                        setSelectedIp(e.target.value)
                        localStorage.setItem('nearby_selected_ip', e.target.value)
                      }}
                      className="font-mono bg-background border border-border rounded-lg p-2 outline-none text-foreground cursor-pointer text-xs"
                    >
                      {localIps.map(ip => (
                        <option key={ip} value={ip}>{ip}</option>
                      ))}
                    </select>
                  </div>

                  {/* 4. Local Wi-Fi Broadcast Toggle */}
                  <div className="p-3 bg-muted/40 border border-border rounded-xl flex items-center justify-between">
                    <div>
                      <span className="font-semibold block text-xs">Local Wi-Fi IPP &amp; mDNS Broadcast</span>
                      <span className="text-[11px] text-muted-foreground">
                        {isBroadcasting ? 'Broadcasting on local network (Port 8631)' : 'Broadcast currently disabled'}
                      </span>
                    </div>
                    <div className="flex items-center gap-2">
                      <span className={`w-2 h-2 rounded-full ${isBroadcasting ? 'bg-emerald-500 animate-pulse' : 'bg-red-500'}`} />
                      <Switch
                        checked={isBroadcasting}
                        onCheckedChange={toggleBroadcast}
                      />
                    </div>
                  </div>

                  {/* 5. Local Wi-Fi Auto-Print Controls */}
                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-2.5">
                    <div className="flex items-center justify-between p-2.5 rounded-xl bg-muted/40 border border-border">
                      <div className="flex items-center gap-2">
                        <Wifi className="w-3.5 h-3.5 text-sky-500" />
                        <span className="text-xs font-semibold">Auto-Print: Wi-Fi Direct</span>
                      </div>
                      <Switch
                        checked={autoPrintWifi}
                        onCheckedChange={setAutoPrintWifi}
                      />
                    </div>
                    <div className="flex items-center justify-between p-2.5 rounded-xl bg-muted/40 border border-border">
                      <div className="flex items-center gap-2">
                        <Globe className="w-3.5 h-3.5 text-purple-500" />
                        <span className="text-xs font-semibold">Auto-Print: Web Portal</span>
                      </div>
                      <Switch
                        checked={autoPrintWeb}
                        onCheckedChange={setAutoPrintWeb}
                      />
                    </div>
                  </div>

                  {/* 6. Wi-Fi Stand Card */}
                  <div className="flex justify-end pt-1">
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={handlePrintLocalQrCard}
                      className="gap-1.5 text-xs h-8"
                    >
                      <Printer className="w-3.5 h-3.5" />
                      Print Local Wi-Fi Standee
                    </Button>
                  </div>
                </div>
              )}

              {/* Tab: Firewall */}
              {settingsTab === 'firewall' && (
                <div className="flex flex-col gap-3">
                  <div className="p-3 bg-muted/40 border border-border rounded-xl flex items-center justify-between">
                    <div>
                      <span className="font-semibold block text-xs">Windows Firewall Port 8631</span>
                      <span className="text-[11px] text-muted-foreground">Required for mobile devices on same Wi-Fi</span>
                    </div>
                    <Button size="sm" onClick={handleFixFirewall} disabled={isFixingFirewall} className="text-xs h-7">
                      {isFixingFirewall ? 'Allowing…' : 'Allow in Firewall'}
                    </Button>
                  </div>
                  {firewallStatus && <div className="text-xs p-2 bg-muted rounded font-mono">{firewallStatus}</div>}
                </div>
              )}

              {/* Tab: Printers List */}
              {settingsTab === 'printers' && (
                <div className="flex flex-col gap-2">
                  {usablePrinters.map(p => (
                    <div key={p.name} className="p-2.5 bg-muted/30 border border-border rounded-lg flex items-center justify-between">
                      <span className="font-medium">{p.name}</span>
                      <span className={`text-[10px] font-semibold px-2 py-0.5 rounded-full ${p.online ? 'bg-emerald-500/15 text-emerald-500' : 'bg-muted text-muted-foreground'}`}>
                        {p.online ? 'Online' : 'Offline'}
                      </span>
                    </div>
                  ))}
                </div>
              )}
            </div>
          </div>
        </div>
      )}
    </div>

    {/* Printable Counter Standee Card (Visible only during Print / PDF Export) */}
    <div id="standee-print-area">
      <div
        style={{
          border: `3px solid ${printStandeeType === 'wifi' ? '#2563eb' : '#4f46e5'}`,
          borderRadius: '24px',
          padding: '38px 28px',
          maxWidth: '460px',
          width: '100%',
          margin: '0 auto',
          textAlign: 'center',
          background: '#ffffff',
          color: '#0f172a',
          fontFamily: 'system-ui, -apple-system, sans-serif',
        }}
      >
        <div
          style={{
            display: 'inline-block',
            background: printStandeeType === 'wifi' ? '#eff6ff' : '#eef2ff',
            color: printStandeeType === 'wifi' ? '#2563eb' : '#4f46e5',
            fontSize: '12px',
            fontWeight: 800,
            padding: '6px 14px',
            borderRadius: '999px',
            marginBottom: '12px',
            border: `1px solid ${printStandeeType === 'wifi' ? '#bfdbfe' : '#c7d2fe'}`,
            letterSpacing: '0.04em',
          }}
        >
          {printStandeeType === 'wifi' ? '📶 LOCAL WI-FI SMART PRINT' : '🌐 SELF-SERVICE SMART PRINT'}
        </div>

        <div style={{ fontSize: '32px', fontWeight: 900, color: '#1e1b4b', margin: '0 0 4px', letterSpacing: '-0.5px' }}>
          NearbyPrint
        </div>

        <div
          style={{
            fontSize: '20px',
            fontWeight: 700,
            color: printStandeeType === 'wifi' ? '#2563eb' : '#4f46e5',
            marginBottom: '22px',
          }}
        >
          {shopName || 'NearbyPrint Counter'}
        </div>

        <div style={{ display: 'flex', justifyContent: 'center', margin: '20px 0' }}>
          <div style={{ padding: '14px', background: '#ffffff', border: '2px solid #e2e8f0', borderRadius: '18px' }}>
            <QRCodeSVG
              value={printStandeeType === 'wifi' ? localPortalUrl : permanentCustomerUrl}
              size={220}
              level="M"
              includeMargin={false}
            />
          </div>
        </div>

        <div style={{ fontSize: '21px', fontWeight: 800, color: '#0f172a', margin: '14px 0 6px' }}>
          Scan to Print — Safe, Easy &amp; Fast
        </div>
        <div style={{ fontSize: '14px', color: '#475569', marginBottom: '18px', fontWeight: 600 }}>
          फ़ोन कैमरा से स्कैन करके सीधे प्रिंट भेजें — सुरक्षित, आसान और तेज़
        </div>

        <div
          style={{
            display: 'flex',
            justifyContent: 'space-around',
            background: '#f8fafc',
            border: '1px solid #e2e8f0',
            borderRadius: '12px',
            padding: '12px 8px',
            margin: '16px 0',
            fontSize: '12px',
            fontWeight: 700,
            color: '#334155',
          }}
        >
          <span>🔒 100% Safe</span>
          <span>✨ Easy 1-Tap</span>
          <span>⚡ Fast Print</span>
        </div>

        <div
          style={{
            fontFamily: 'monospace',
            fontSize: '12px',
            color: printStandeeType === 'wifi' ? '#2563eb' : '#4f46e5',
            background: printStandeeType === 'wifi' ? '#eff6ff' : '#f5f3ff',
            border: `1px solid ${printStandeeType === 'wifi' ? '#bfdbfe' : '#ddd6fe'}`,
            padding: '8px 14px',
            borderRadius: '8px',
            display: 'inline-block',
            wordBreak: 'break-all',
            marginTop: '6px',
            fontWeight: 600,
          }}
        >
          {printStandeeType === 'wifi' ? localPortalUrl : permanentCustomerUrl}
        </div>

        <div style={{ marginTop: '22px', fontSize: '11px', color: '#64748b', fontWeight: 500 }}>
          Files are automatically deleted immediately after printing.
        </div>
      </div>
    </div>
    </>
  )
}
