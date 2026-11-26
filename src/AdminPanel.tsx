import { useEffect, useMemo, useState } from 'react'
import {
  ShieldCheck, Store, Printer, Activity, Calendar, Lock, LogOut,
  RefreshCw, Search, AlertTriangle, BarChart3
} from 'lucide-react'
import {
  ResponsiveContainer, AreaChart, Area, XAxis, YAxis, CartesianGrid,
  Tooltip, BarChart, Bar, Legend
} from 'recharts'
import { supabase, isShopLive } from './supabaseClient'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'

const ADMIN_EMAIL = 'menootankumar@gmail.com'

type AdminShopRow = {
  id?: string
  shop_unique_id: string
  shop_name: string
  owner_name?: string
  last_active_at?: string | null
  created_at?: string
}

// Strict privacy: never query file_url or customer_name in Admin Panel
type AdminJobMeta = {
  id: string
  shop_unique_id: string
  status: string
  created_at?: string
}

export default function AdminPanel() {
  const [authLoading, setAuthLoading] = useState(true)
  const [userEmail, setUserEmail] = useState<string | null>(null)
  const [emailInput, setEmailInput] = useState('')
  const [passwordInput, setPasswordInput] = useState('')
  const [authError, setAuthError] = useState('')
  const [isSigningIn, setIsSigningIn] = useState(false)

  const [shops, setShops] = useState<AdminShopRow[]>([])
  const [jobs, setJobs] = useState<AdminJobMeta[]>([])
  const [isLoadingData, setIsLoadingData] = useState(false)
  const [dataError, setDataError] = useState('')
  const [searchQuery, setSearchQuery] = useState('')

  useEffect(() => {
    supabase.auth.getSession().then(({ data }) => {
      setUserEmail(data.session?.user?.email?.toLowerCase() || null)
      setAuthLoading(false)
    })

    const { data: sub } = supabase.auth.onAuthStateChange((_event, session) => {
      setUserEmail(session?.user?.email?.toLowerCase() || null)
      setAuthLoading(false)
    })

    return () => {
      sub.subscription.unsubscribe()
    }
  }, [])

  const isAuthorizedAdmin = userEmail === ADMIN_EMAIL

  const fetchAdminData = async () => {
    if (!isAuthorizedAdmin) return
    setIsLoadingData(true)
    setDataError('')
    try {
      const [shopsRes, jobsRes] = await Promise.all([
        supabase
          .from('shops')
          .select('id, shop_unique_id, shop_name, owner_name, last_active_at, created_at')
          .order('created_at', { ascending: false }),
        supabase
          .from('print_jobs')
          .select('id, shop_unique_id, status, created_at')
          .order('created_at', { ascending: false })
          .limit(5000),
      ])

      if (shopsRes.error) {
        // Fallback if last_active_at column hasn't been added yet
        const fallbackShops = await supabase
          .from('shops')
          .select('id, shop_unique_id, shop_name, owner_name, created_at')
          .order('created_at', { ascending: false })
        if (fallbackShops.error) throw fallbackShops.error
        setShops((fallbackShops.data || []) as AdminShopRow[])
      } else {
        setShops((shopsRes.data || []) as AdminShopRow[])
      }

      if (jobsRes.error) throw jobsRes.error
      setJobs((jobsRes.data || []) as AdminJobMeta[])
    } catch (err: unknown) {
      console.error('Admin fetch error:', err)
      setDataError(err instanceof Error ? err.message : String(err))
    } finally {
      setIsLoadingData(false)
    }
  }

  useEffect(() => {
    if (isAuthorizedAdmin) {
      fetchAdminData()
      const interval = setInterval(fetchAdminData, 30_000)
      return () => clearInterval(interval)
    }
  }, [isAuthorizedAdmin])

  const handleLogin = async (e: React.FormEvent) => {
    e.preventDefault()
    setAuthError('')
    setIsSigningIn(true)
    try {
      const { data, error } = await supabase.auth.signInWithPassword({
        email: emailInput.trim(),
        password: passwordInput,
      })
      if (error) throw error
      const signedEmail = data.user?.email?.toLowerCase() || ''
      if (signedEmail !== ADMIN_EMAIL) {
        setAuthError('Access Denied: This email is not authorized for Super Admin access.')
      }
    } catch (err: unknown) {
      setAuthError(err instanceof Error ? err.message : 'Login failed')
    } finally {
      setIsSigningIn(false)
    }
  }

  const handleSignOut = async () => {
    await supabase.auth.signOut()
    setUserEmail(null)
  }

  // Compute KPIs, 7-Day Daily Volume, and Shop-wise Usage (counts only)
  const metrics = useMemo(() => {
    const now = new Date()
    const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime()

    const printedJobs = jobs.filter(j => j.status === 'printed')
    const todaysPrints = printedJobs.filter(j => j.created_at && new Date(j.created_at).getTime() >= startOfToday).length
    const lifetimePrints = printedJobs.length
    const liveShopsCount = shops.filter(s => isShopLive(s.last_active_at)).length

    // Last 7 days daily volume
    const dailyMap = new Map<string, { date: string; prints: number; totalJobs: number }>()
    for (let i = 6; i >= 0; i--) {
      const d = new Date(now)
      d.setDate(now.getDate() - i)
      const key = d.toISOString().slice(0, 10)
      const label = d.toLocaleDateString('en-IN', { day: '2-digit', month: 'short' })
      dailyMap.set(key, { date: label, prints: 0, totalJobs: 0 })
    }

    for (const j of jobs) {
      if (!j.created_at) continue
      const key = new Date(j.created_at).toISOString().slice(0, 10)
      const bucket = dailyMap.get(key)
      if (bucket) {
        bucket.totalJobs += 1
        if (j.status === 'printed') {
          bucket.prints += 1
        }
      }
    }

    const dailyChartData = Array.from(dailyMap.values())

    // Shop-wise counts
    const shopCounts = new Map<string, { today: number; total: number }>()
    for (const j of printedJobs) {
      const sid = (j.shop_unique_id || '').toLowerCase()
      if (!sid) continue
      const cur = shopCounts.get(sid) || { today: 0, total: 0 }
      cur.total += 1
      if (j.created_at && new Date(j.created_at).getTime() >= startOfToday) {
        cur.today += 1
      }
      shopCounts.set(sid, cur)
    }

    const shopRows = shops.map(s => {
      const sid = (s.shop_unique_id || '').toLowerCase()
      const counts = shopCounts.get(sid) || { today: 0, total: 0 }
      return {
        ...s,
        live: isShopLive(s.last_active_at),
        todayPrints: counts.today,
        totalPrints: counts.total,
      }
    })

    const topShopsChartData = [...shopRows]
      .sort((a, b) => b.totalPrints - a.totalPrints)
      .slice(0, 7)
      .map(s => ({
        name: s.shop_name?.slice(0, 14) || s.shop_unique_id,
        Today: s.todayPrints,
        Total: s.totalPrints,
      }))

    return {
      totalShops: shops.length,
      liveShopsCount,
      todaysPrints,
      lifetimePrints,
      dailyChartData,
      topShopsChartData,
      shopRows,
    }
  }, [shops, jobs])

  const filteredShops = useMemo(() => {
    const q = searchQuery.trim().toLowerCase()
    if (!q) return metrics.shopRows
    return metrics.shopRows.filter(
      s =>
        s.shop_name?.toLowerCase().includes(q) ||
        s.shop_unique_id?.toLowerCase().includes(q) ||
        s.owner_name?.toLowerCase().includes(q)
    )
  }, [metrics.shopRows, searchQuery])

  if (authLoading) {
    return (
      <div className="min-h-screen bg-[#090d16] flex flex-col items-center justify-center gap-4">
        <div className="w-[84px] h-[84px] rounded-[20px] bg-white flex items-center justify-center shadow-2xl">
          <div className="loader"></div>
        </div>
        <span className="text-xs font-semibold text-slate-400 uppercase tracking-wider">Verifying Admin Session…</span>
      </div>
    )
  }

  if (!userEmail) {
    return (
      <div className="min-h-screen bg-[#090d16] text-slate-100 flex items-center justify-center p-4">
        <form
          onSubmit={handleLogin}
          className="w-full max-w-sm bg-[#131b2e] border border-[#1e293b] rounded-2xl p-6 shadow-2xl flex flex-col gap-4"
        >
          <div className="flex items-center gap-3">
            <div className="p-2.5 rounded-xl bg-indigo-500/15 text-indigo-400 border border-indigo-500/30">
              <ShieldCheck className="w-6 h-6" />
            </div>
            <div>
              <h1 className="text-lg font-bold">NearbyPrint Super Admin</h1>
              <p className="text-xs text-slate-400">Privacy-First Analytics Console</p>
            </div>
          </div>

          {authError && (
            <div className="p-3 rounded-xl bg-red-500/10 border border-red-500/30 text-red-400 text-xs font-medium">
              {authError}
            </div>
          )}

          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-semibold text-slate-300">Admin Email</label>
            <Input
              type="email"
              required
              value={emailInput}
              onChange={e => setEmailInput(e.target.value)}
              placeholder="menootankumar@gmail.com"
              className="bg-[#090d16] border-[#1e293b] text-xs"
            />
          </div>

          <div className="flex flex-col gap-1.5">
            <label className="text-xs font-semibold text-slate-300">Password</label>
            <Input
              type="password"
              required
              value={passwordInput}
              onChange={e => setPasswordInput(e.target.value)}
              placeholder="••••••••"
              className="bg-[#090d16] border-[#1e293b] text-xs"
            />
          </div>

          <Button
            type="submit"
            disabled={isSigningIn}
            className="w-full bg-indigo-600 hover:bg-indigo-700 text-white font-semibold text-xs h-10 mt-1"
          >
            {isSigningIn ? 'Signing In…' : 'Sign In to Super Admin'}
          </Button>
        </form>
      </div>
    )
  }

  if (!isAuthorizedAdmin) {
    return (
      <div className="min-h-screen bg-[#090d16] text-slate-100 flex items-center justify-center p-4">
        <div className="max-w-sm w-full bg-[#131b2e] border border-red-500/30 rounded-2xl p-6 text-center flex flex-col items-center gap-4">
          <div className="p-3 rounded-full bg-red-500/15 text-red-400">
            <Lock className="w-7 h-7" />
          </div>
          <div>
            <h2 className="text-base font-bold text-red-400">Access Denied</h2>
            <p className="text-xs text-slate-400 mt-1">
              Signed in as <span className="font-mono text-slate-200">{userEmail}</span>. Only the authorized Super Admin can view this panel.
            </p>
          </div>
          <Button variant="outline" size="sm" onClick={handleSignOut} className="gap-2 text-xs">
            <LogOut className="w-3.5 h-3.5" /> Sign Out
          </Button>
        </div>
      </div>
    )
  }

  return (
    <div className="min-h-screen bg-[#090d16] text-slate-100 flex flex-col">
      {/* Header */}
      <header className="sticky top-0 z-30 bg-[#131b2e]/90 backdrop-blur-md border-b border-[#1e293b] px-6 py-3.5 flex items-center justify-between">
        <div className="flex items-center gap-3">
          <div className="p-2 rounded-xl bg-indigo-500/15 text-indigo-400 border border-indigo-500/30">
            <ShieldCheck className="w-5 h-5" />
          </div>
          <div>
            <div className="flex items-center gap-2">
              <h1 className="text-base font-bold tracking-tight">NearbyPrint Super Admin</h1>
              <span className="text-[10px] font-bold px-2 py-0.5 rounded-full bg-emerald-500/15 text-emerald-400 border border-emerald-500/30 flex items-center gap-1">
                <Lock className="w-2.5 h-2.5" /> Privacy-First
              </span>
            </div>
            <p className="text-[11px] text-slate-400">Counts &amp; shop metadata only — zero document access</p>
          </div>
        </div>

        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={fetchAdminData}
            disabled={isLoadingData}
            className="gap-1.5 text-xs h-8 bg-[#090d16] border-[#1e293b]"
          >
            <RefreshCw className={`w-3.5 h-3.5 ${isLoadingData ? 'animate-spin' : ''}`} />
            Refresh
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={handleSignOut}
            className="gap-1.5 text-xs h-8 text-slate-400 hover:text-white"
          >
            <LogOut className="w-3.5 h-3.5" /> Sign Out
          </Button>
        </div>
      </header>

      <main className="max-w-6xl mx-auto w-full p-5 flex flex-col gap-6">
        {dataError && (
          <div className="p-3 rounded-xl bg-red-500/10 border border-red-500/30 text-red-400 text-xs flex items-center gap-2">
            <AlertTriangle className="w-4 h-4 shrink-0" />
            <span>{dataError}</span>
          </div>
        )}

        {/* Top KPI Cards */}
        <section className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4">
          <div className="bg-[#131b2e] border border-[#1e293b] rounded-2xl p-4 flex items-center justify-between">
            <div>
              <span className="text-xs font-medium text-slate-400">Total Registered Shops</span>
              <div className="text-2xl font-extrabold mt-1">{metrics.totalShops}</div>
            </div>
            <div className="p-3 rounded-xl bg-indigo-500/15 text-indigo-400">
              <Store className="w-5 h-5" />
            </div>
          </div>

          <div className="bg-[#131b2e] border border-[#1e293b] rounded-2xl p-4 flex items-center justify-between">
            <div>
              <span className="text-xs font-medium text-slate-400 flex items-center gap-1.5">
                <span className="w-2 h-2 rounded-full bg-emerald-500 animate-pulse" />
                Live Active Shops
              </span>
              <div className="text-2xl font-extrabold text-emerald-400 mt-1">{metrics.liveShopsCount}</div>
            </div>
            <div className="p-3 rounded-xl bg-emerald-500/15 text-emerald-400">
              <Activity className="w-5 h-5" />
            </div>
          </div>

          <div className="bg-[#131b2e] border border-[#1e293b] rounded-2xl p-4 flex items-center justify-between">
            <div>
              <span className="text-xs font-medium text-slate-400">Today&apos;s Prints</span>
              <div className="text-2xl font-extrabold text-sky-400 mt-1">{metrics.todaysPrints}</div>
            </div>
            <div className="p-3 rounded-xl bg-sky-500/15 text-sky-400">
              <Calendar className="w-5 h-5" />
            </div>
          </div>

          <div className="bg-[#131b2e] border border-[#1e293b] rounded-2xl p-4 flex items-center justify-between">
            <div>
              <span className="text-xs font-medium text-slate-400">Lifetime Total Prints</span>
              <div className="text-2xl font-extrabold text-purple-400 mt-1">{metrics.lifetimePrints}</div>
            </div>
            <div className="p-3 rounded-xl bg-purple-500/15 text-purple-400">
              <Printer className="w-5 h-5" />
            </div>
          </div>
        </section>

        {/* Visual Charts */}
        <section className="grid grid-cols-1 lg:grid-cols-2 gap-5">
          {/* Chart 1: Daily Print Volume */}
          <div className="bg-[#131b2e] border border-[#1e293b] rounded-2xl p-5 flex flex-col gap-3">
            <div className="flex items-center justify-between">
              <div>
                <h2 className="text-sm font-bold flex items-center gap-2">
                  <BarChart3 className="w-4 h-4 text-indigo-400" />
                  Daily Print Volume (Last 7 Days)
                </h2>
                <p className="text-[11px] text-slate-400">Printed documents vs total submitted jobs</p>
              </div>
            </div>
            <div className="h-64 w-full">
              <ResponsiveContainer width="100%" height="100%">
                <AreaChart data={metrics.dailyChartData} margin={{ top: 10, right: 12, left: -16, bottom: 0 }}>
                  <defs>
                    <linearGradient id="colorPrints" x1="0" y1="0" x2="0" y2="1">
                      <stop offset="5%" stopColor="#6366f1" stopOpacity={0.45} />
                      <stop offset="95%" stopColor="#6366f1" stopOpacity={0} />
                    </linearGradient>
                  </defs>
                  <CartesianGrid strokeDasharray="3 3" stroke="#1e293b" />
                  <XAxis dataKey="date" stroke="#64748b" fontSize={11} />
                  <YAxis stroke="#64748b" fontSize={11} allowDecimals={false} />
                  <Tooltip
                    contentStyle={{ backgroundColor: '#0f172a', borderColor: '#1e293b', borderRadius: '10px', fontSize: '12px' }}
                  />
                  <Area type="monotone" dataKey="prints" name="Printed" stroke="#6366f1" strokeWidth={2.5} fillOpacity={1} fill="url(#colorPrints)" />
                </AreaChart>
              </ResponsiveContainer>
            </div>
          </div>

          {/* Chart 2: Shop Activity & Usage */}
          <div className="bg-[#131b2e] border border-[#1e293b] rounded-2xl p-5 flex flex-col gap-3">
            <div>
              <h2 className="text-sm font-bold flex items-center gap-2">
                <Store className="w-4 h-4 text-emerald-400" />
                Shop Activity &amp; Usage (Top Shops)
              </h2>
              <p className="text-[11px] text-slate-400">Today&apos;s prints vs Lifetime total prints</p>
            </div>
            <div className="h-64 w-full">
              <ResponsiveContainer width="100%" height="100%">
                <BarChart data={metrics.topShopsChartData} margin={{ top: 10, right: 12, left: -16, bottom: 0 }}>
                  <CartesianGrid strokeDasharray="3 3" stroke="#1e293b" />
                  <XAxis dataKey="name" stroke="#64748b" fontSize={11} />
                  <YAxis stroke="#64748b" fontSize={11} allowDecimals={false} />
                  <Tooltip
                    contentStyle={{ backgroundColor: '#0f172a', borderColor: '#1e293b', borderRadius: '10px', fontSize: '12px' }}
                  />
                  <Legend wrapperStyle={{ fontSize: '11px' }} />
                  <Bar dataKey="Today" fill="#38bdf8" radius={[4, 4, 0, 0]} />
                  <Bar dataKey="Total" fill="#10b981" radius={[4, 4, 0, 0]} />
                </BarChart>
              </ResponsiveContainer>
            </div>
          </div>
        </section>

        {/* Shop-wise List */}
        <section className="bg-[#131b2e] border border-[#1e293b] rounded-2xl p-5 flex flex-col gap-4">
          <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3">
            <div>
              <h2 className="text-sm font-bold">Registered Shops Directory</h2>
              <p className="text-[11px] text-slate-400">Live status based on 90-second desktop heartbeat</p>
            </div>
            <div className="relative w-full sm:w-64">
              <Search className="w-3.5 h-3.5 text-slate-400 absolute left-3 top-1/2 -translate-y-1/2" />
              <Input
                value={searchQuery}
                onChange={e => setSearchQuery(e.target.value)}
                placeholder="Search shop name or ID..."
                className="pl-8 bg-[#090d16] border-[#1e293b] text-xs h-8"
              />
            </div>
          </div>

          <div className="overflow-x-auto">
            <table className="w-full text-left border-collapse text-xs">
              <thead>
                <tr className="border-b border-[#1e293b] text-slate-400 uppercase text-[10px] tracking-wider">
                  <th className="py-2.5 px-3">Shop Name</th>
                  <th className="py-2.5 px-3">Shop ID</th>
                  <th className="py-2.5 px-3">Status</th>
                  <th className="py-2.5 px-3 text-right">Today&apos;s Prints</th>
                  <th className="py-2.5 px-3 text-right">Total Prints</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-[#1e293b]/60">
                {filteredShops.length === 0 ? (
                  <tr>
                    <td colSpan={5} className="py-8 text-center text-slate-500">
                      No shops found.
                    </td>
                  </tr>
                ) : (
                  filteredShops.map(shop => (
                    <tr key={shop.shop_unique_id} className="hover:bg-[#090d16]/50 transition-colors">
                      <td className="py-3 px-3 font-semibold text-slate-100">
                        {shop.shop_name || 'Unnamed Shop'}
                        {shop.owner_name && (
                          <span className="block text-[10px] text-slate-400 font-normal">{shop.owner_name}</span>
                        )}
                      </td>
                      <td className="py-3 px-3 font-mono text-indigo-400">{shop.shop_unique_id}</td>
                      <td className="py-3 px-3">
                        {shop.live ? (
                          <span className="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full bg-emerald-500/15 text-emerald-400 border border-emerald-500/30 text-[10px] font-bold">
                            <span className="w-1.5 h-1.5 rounded-full bg-emerald-400 animate-pulse" />
                            Online
                          </span>
                        ) : (
                          <span className="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full bg-slate-500/15 text-slate-400 border border-slate-500/20 text-[10px] font-semibold">
                            <span className="w-1.5 h-1.5 rounded-full bg-slate-500" />
                            Offline
                          </span>
                        )}
                      </td>
                      <td className="py-3 px-3 text-right font-bold text-sky-400">{shop.todayPrints}</td>
                      <td className="py-3 px-3 text-right font-bold text-emerald-400">{shop.totalPrints}</td>
                    </tr>
                  ))
                )}
              </tbody>
            </table>
          </div>
        </section>
      </main>
    </div>
  )
}
