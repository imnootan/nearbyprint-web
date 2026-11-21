import React, { useState, useEffect, useRef, useCallback } from 'react'
import { supabase, ShopRecord } from './supabaseClient'

// -------------------------------------------------------------
// Bilingual Localization (Professional English & Pure Hindi)
// -------------------------------------------------------------
const I18N = {
  en: {
    portalTitle: 'NearbyPrint Self-Service',
    langLabel: 'हिन्दी',
    cloudBadge: 'Cloud Online',
    customerNameLabel: 'Customer Name',
    customerNameHint: '(Required for print identification)',
    customerPlaceholder: 'e.g. Rahul Sharma',
    tabCard: 'Aadhaar / PAN (CR80)',
    tabDoc: 'Document / Photos',
    cardLayoutLabel: 'Card Print Layout',
    layoutBoth: 'Front & Back',
    layoutFront: 'Front Only',
    layoutBack: 'Back Only',
    placementLabel: 'Xerox Page Placement',
    placeTop: 'Upper Half (Standard Xerox)',
    placeCenter: 'Center of Page',
    docSidesLabel: 'Print Sides',
    sideSingle: 'Single-Sided (1-Side)',
    sideDouble: 'Double-Sided (Duplex)',
    paperSizeLabel: 'Paper Size',
    orientationLabel: 'Orientation',
    portrait: 'Portrait',
    landscape: 'Landscape',
    frontTitle: 'Front Side',
    frontHint: 'Tap to upload',
    backTitle: 'Back Side',
    backHint: 'Tap to upload',
    cropBtn: '✂️ Crop',
    a4PreviewTitle: 'A4 Xerox Sheet Preview',
    a4PreviewSub: 'Standard CR80 Layout',
    a4LayoutDesc: 'Both Front & Back cards are automatically aligned in the upper half of A4 page.',
    uploadDocTitle: 'Upload Document or Photo',
    uploadDocSub: 'PDF, JPG, PNG (Max 50MB)',
    filterLabel: 'Document Enhancement Filter',
    filterSub: 'Adobe Scan Filter',
    filterBwTitle: 'Crisp B&W',
    filterBwSub: '(Shadow-free)',
    filterColorTitle: 'Magic Color',
    filterColorSub: '(Clean paper)',
    filterOrigTitle: 'Original',
    filterOrigSub: '(As captured)',
    copiesLabel: 'Copies',
    colorModeLabel: 'Print Color',
    btnBw: 'B&W',
    btnColor: 'Color',
    submitBtn: '🚀 Send to Printer',
    submitting: 'Submitting Job…',
    uploadingFile: 'Uploading to Cloud…',
    privacyFooter: '🔒 Files are automatically deleted from server memory immediately after printing.',
    successTitle: 'Print Job Submitted Successfully!',
    successSub: 'Your document has been sent directly to the counter printer.',
    resCustomerLabel: 'Customer:',
    resDetailsLabel: 'Print Details:',
    resStatusLabel: 'Status:',
    resStatusVal: 'Waiting for Counter Print',
    resShopLabel: 'Shop:',
    printAnotherBtn: '+ Print Another Document',
    cropHeader: '📐 Adobe Corner Crop & Straighten',
    cropAutoBtn: '🎯 Auto Detect',
    cropResetBtn: '🔄 Reset',
    cropApplyBtn: '✓ Crop & Apply',
    btnChange: '🔄 Change',
    frontCardPlaceholder: 'Front (Top)',
    backCardPlaceholder: 'Back (Bottom)',
    alertEnterName: 'Please enter your name.',
    alertUploadCard: 'Please upload at least one side of your Aadhaar or PAN card.',
    alertSelectDoc: 'Please select a document or image to print.',
    alertUploadFail: 'Print upload failed. Please try again.',
  },
  hi: {
    portalTitle: 'NearbyPrint सेल्फ़-सर्विस',
    langLabel: 'English',
    cloudBadge: 'क्लाउड ऑनलाइन',
    customerNameLabel: 'ग्राहक का नाम',
    customerNameHint: '(प्रिंट पहचान के लिए आवश्यक)',
    customerPlaceholder: 'जैसे: राहुल शर्मा',
    tabCard: 'आधार / पैन कार्ड (CR80)',
    tabDoc: 'दस्तावेज़ / फ़ोटो',
    cardLayoutLabel: 'कार्ड प्रिंट लेआउट',
    layoutBoth: 'सामने और पीछे (दोनों)',
    layoutFront: 'केवल सामने (Front Only)',
    layoutBack: 'केवल पीछे (Back Only)',
    placementLabel: 'जेरॉक्स स्थिति (Placement)',
    placeTop: 'ऊपरी आधा भाग (मानक जेरॉक्स)',
    placeCenter: 'कागज़ के बीच में',
    docSidesLabel: 'प्रिंट की सतह',
    sideSingle: 'एक तरफ (Single Sided)',
    sideDouble: 'दोनों तरफ (Duplex)',
    paperSizeLabel: 'कागज़ का साइज़',
    orientationLabel: 'दिशा (Orientation)',
    portrait: 'लंबवत (Portrait)',
    landscape: 'क्षैतिज (Landscape)',
    frontTitle: 'सामने का भाग (Front)',
    frontHint: 'फ़ोटो अपलोड करें',
    backTitle: 'पीछे का भाग (Back)',
    backHint: 'फ़ोटो अपलोड करें',
    cropBtn: '✂️ क्रॉप करें',
    a4PreviewTitle: 'A4 जेरॉक्स शीट प्रीव्यू',
    a4PreviewSub: 'मानक CR80 लेआउट',
    a4LayoutDesc: 'सामने और पीछे के दोनों भाग A4 शीट के ऊपरी हिस्से में स्वचालित रूप से व्यवस्थित हैं।',
    uploadDocTitle: 'दस्तावेज़ या फ़ोटो अपलोड करें',
    uploadDocSub: 'PDF, JPG, PNG (अधिकतम 50MB)',
    filterLabel: 'दस्तावेज़ सुधार फ़िल्टर',
    filterSub: 'अडोब स्कैन फ़िल्टर',
    filterBwTitle: 'स्पष्ट B&W',
    filterBwSub: '(छाया रहित)',
    filterColorTitle: 'मैजिक कलर',
    filterColorSub: '(साफ़ कागज़)',
    filterOrigTitle: 'मूल प्रति',
    filterOrigSub: '(जैसा खींचा गया)',
    copiesLabel: 'प्रतियों की संख्या (Copies)',
    colorModeLabel: 'प्रिंट का प्रकार',
    btnBw: 'ब्लैक & व्हाइट',
    btnColor: 'रंगीन (Color)',
    submitBtn: '🚀 प्रिंट के लिए भेजें',
    submitting: 'भेजा जा रहा है…',
    uploadingFile: 'क्लाउड पर अपलोड हो रहा है…',
    privacyFooter: '🔒 प्रिंट होते ही फ़ाइल सिस्टम से स्वतः हमेशा के लिए हटा दी जाती है।',
    successTitle: 'प्रिंट सफलतापूर्वक भेजा गया!',
    successSub: 'आपका दस्तावेज़ सीधे काउंटर प्रिंटर को भेज दिया गया है।',
    resCustomerLabel: 'ग्राहक:',
    resDetailsLabel: 'प्रिंट विवरण:',
    resStatusLabel: 'स्थिति:',
    resStatusVal: 'काउंटर प्रिंट की प्रतीक्षा में',
    resShopLabel: 'दुकान:',
    printAnotherBtn: '+ दूसरा दस्तावेज़ प्रिंट करें',
    cropHeader: '📐 कॉर्नर क्रॉप और सीधा करें',
    cropAutoBtn: '🎯 स्वतः पहचान',
    cropResetBtn: '🔄 रीसेट',
    cropApplyBtn: '✓ क्रॉप लागू करें',
    btnChange: '🔄 बदलें',
    frontCardPlaceholder: 'सामने (ऊपर)',
    backCardPlaceholder: 'पीछे (नीचे)',
    alertEnterName: 'कृपया अपना नाम दर्ज करें।',
    alertUploadCard: 'कृपया आधार या पैन कार्ड का कम से कम एक भाग अपलोड करें।',
    alertSelectDoc: 'कृपया प्रिंट करने के लिए कोई दस्तावेज़ या फ़ोटो चुनें।',
    alertUploadFail: 'प्रिंट अपलोड विफल रहा। कृपया पुनः प्रयास करें।',
  }
}

// -------------------------------------------------------------
// Adobe Scan Local Adaptive Threshold & Shadow Removal Filter
// -------------------------------------------------------------
function applyAdobeScanFilter(canvas: HTMLCanvasElement, filterType: 'bw' | 'color' | 'original') {
  if (filterType === 'original') return
  const ctx = canvas.getContext('2d')
  if (!ctx) return
  const w = canvas.width
  const h = canvas.height
  const imgData = ctx.getImageData(0, 0, w, h)
  const data = imgData.data

  const len = w * h
  const gray = new Uint8Array(len)
  for (let i = 0, j = 0; i < data.length; i += 4, j++) {
    gray[j] = Math.round(0.299 * data[i] + 0.587 * data[i + 1] + 0.114 * data[i + 2])
  }

  // Fast Summed Area Table (Integral Image) for O(1) local mean
  const rowW = w + 1
  const integral = new Float64Array(rowW * (h + 1))
  for (let y = 0; y < h; y++) {
    let rowSum = 0
    const yRow = (y + 1) * rowW
    const prevYRow = y * rowW
    const gRow = y * w
    for (let x = 0; x < w; x++) {
      rowSum += gray[gRow + x]
      integral[yRow + x + 1] = integral[prevYRow + x + 1] + rowSum
    }
  }

  const radius = Math.max(14, Math.min(36, Math.round(Math.min(w, h) / 32)))

  for (let y = 0; y < h; y++) {
    const y0 = Math.max(0, y - radius)
    const y1 = Math.min(h, y + radius + 1)
    const gRow = y * w
    const dRow = gRow * 4

    for (let x = 0; x < w; x++) {
      const x0 = Math.max(0, x - radius)
      const x1 = Math.min(w, x + radius + 1)
      const count = (x1 - x0) * (y1 - y0)
      const sum = integral[y1 * rowW + x1] - integral[y0 * rowW + x1] - integral[y1 * rowW + x0] + integral[y0 * rowW + x0]
      const localMean = sum / count

      const idx = dRow + (x * 4)
      const g = gray[gRow + x]
      const ratio = g / (localMean + 1e-4)

      if (filterType === 'bw') {
        let val: number
        if (ratio >= 0.88) {
          val = 255
        } else if (ratio <= 0.65) {
          val = Math.max(0, Math.round((ratio / 0.65) * 45))
        } else {
          val = Math.round(45 + ((ratio - 0.65) / 0.23) * 210)
        }
        data[idx] = val
        data[idx + 1] = val
        data[idx + 2] = val
      } else if (filterType === 'color') {
        const factor = ratio >= 0.88 ? (255 / Math.max(1, g)) : Math.min(1.4, 1.0 / Math.max(0.45, ratio))
        data[idx] = Math.min(255, Math.round(data[idx] * factor))
        data[idx + 1] = Math.min(255, Math.round(data[idx + 1] * factor))
        data[idx + 2] = Math.min(255, Math.round(data[idx + 2] * factor))
      }
    }
  }

  ctx.putImageData(imgData, 0, 0)
}

// -------------------------------------------------------------
// Closed-Form Projective Homography (cv2.warpPerspective)
// -------------------------------------------------------------
interface Point2D {
  x: number
  y: number
}

function warpPerspective(
  srcImg: CanvasImageSource,
  srcW: number,
  srcH: number,
  corners: { tl: Point2D; tr: Point2D; br: Point2D; bl: Point2D },
  outW: number,
  outH: number
): HTMLCanvasElement {
  const tempSrc = document.createElement('canvas')
  tempSrc.width = srcW
  tempSrc.height = srcH
  const sCtx = tempSrc.getContext('2d')!
  sCtx.drawImage(srcImg, 0, 0)
  const sData = sCtx.getImageData(0, 0, srcW, srcH)

  const dst = document.createElement('canvas')
  dst.width = outW
  dst.height = outH
  const dCtx = dst.getContext('2d')!
  const dData = dCtx.createImageData(outW, outH)

  const sp = sData.data
  const dp = dData.data
  const { tl, tr, br, bl } = corners

  // Heckbert's Projective Mapping: Unit square (u_n, v_n) -> Quad (sxPt, syPt)
  const dx1 = tr.x - br.x
  const dx2 = bl.x - br.x
  const dy1 = tr.y - br.y
  const dy2 = bl.y - br.y
  const sx = tl.x - tr.x + br.x - bl.x
  const sy = tl.y - tr.y + br.y - bl.y

  let g = 0, h = 0
  const det = dx1 * dy2 - dx2 * dy1
  if (Math.abs(det) > 1e-7) {
    g = (sx * dy2 - sy * dx2) / det
    h = (dx1 * sy - dy1 * sx) / det
  }
  const a = tr.x - tl.x + g * tr.x
  const b = bl.x - tl.x + h * bl.x
  const c = tl.x
  const d = tr.y - tl.y + g * tr.y
  const e = bl.y - tl.y + h * bl.y
  const f = tl.y

  for (let y = 0; y < outH; y++) {
    const vn = y / (outH - 1)
    const dRow = y * outW * 4

    for (let x = 0; x < outW; x++) {
      const un = x / (outW - 1)
      const denom = g * un + h * vn + 1
      const sxPt = (a * un + b * vn + c) / denom
      const syPt = (d * un + e * vn + f) / denom

      const x0 = Math.floor(sxPt)
      const y0 = Math.floor(syPt)
      const x1 = Math.min(srcW - 1, x0 + 1)
      const y1 = Math.min(srcH - 1, y0 + 1)
      const dx = sxPt - x0
      const dy = syPt - y0

      if (x0 >= 0 && x1 < srcW && y0 >= 0 && y1 < srcH) {
        const i00 = (y0 * srcW + x0) * 4
        const i10 = (y0 * srcW + x1) * 4
        const i01 = (y1 * srcW + x0) * 4
        const i11 = (y1 * srcW + x1) * 4
        const dIdx = dRow + (x * 4)

        for (let ch = 0; ch < 4; ch++) {
          dp[dIdx + ch] = Math.round(
            (1 - dx) * (1 - dy) * sp[i00 + ch] +
            dx * (1 - dy) * sp[i10 + ch] +
            (1 - dx) * dy * sp[i01 + ch] +
            dx * dy * sp[i11 + ch]
          )
        }
      }
    }
  }

  dCtx.putImageData(dData, 0, 0)
  return dst
}

export default function CustomerUploadPortal() {
  // Localization
  const [lang, setLang] = useState<'en' | 'hi'>(() => {
    const saved = localStorage.getItem('nearby_portal_lang')
    if (saved === 'en' || saved === 'hi') return saved
    return (typeof navigator !== 'undefined' && navigator.language && navigator.language.toLowerCase().startsWith('hi')) ? 'hi' : 'en'
  })
  const t = I18N[lang]

  const toggleLanguage = () => {
    const next = lang === 'en' ? 'hi' : 'en'
    setLang(next)
    localStorage.setItem('nearby_portal_lang', next)
  }

  // Shop Context from URL
  const [shopId, setShopId] = useState<string>('nootan_prints')
  const [shopDetails, setShopDetails] = useState<ShopRecord | null>(null)

  useEffect(() => {
    let resolved = ''
    try {
      const search = new URLSearchParams(window.location.search)
      resolved = search.get('shop') || ''
      if (!resolved && window.location.hash.includes('?')) {
        const hashQuery = window.location.hash.split('?')[1]
        const hashParams = new URLSearchParams(hashQuery)
        resolved = hashParams.get('shop') || ''
      }
      if (!resolved && window.location.pathname.startsWith('/shop/')) {
        resolved = window.location.pathname.replace('/shop/', '').split('/')[0]
      }
    } catch {
      resolved = ''
    }

    if (!resolved) {
      resolved = localStorage.getItem('nearby_last_visited_shop') || 'nootan_prints'
    }
    setShopId(resolved)
    localStorage.setItem('nearby_last_visited_shop', resolved)

    // Load shop details
    const fetchShop = async () => {
      try {
        const { data } = await supabase
          .from('shops')
          .select('*')
          .eq('shop_unique_id', resolved)
          .maybeSingle()

        if (data) {
          setShopDetails(data)
        } else {
          setShopDetails({
            shop_unique_id: resolved,
            shop_name: resolved.replace(/_/g, ' ').toUpperCase(),
            owner_name: 'Store Counter'
          })
        }
      } catch {
        // Fallback default
      }
    }
    fetchShop()
  }, [])

  // Form State
  const [customerName, setCustomerName] = useState<string>('')
  const [mode, setMode] = useState<'card' | 'doc'>('card')
  const [cardLayout, setCardLayout] = useState<'both' | 'front_only' | 'back_only'>('both')
  const [placement, setPlacement] = useState<'top' | 'center'>('top')
  const [paperSize, setPaperSize] = useState<'A4' | 'Legal' | '4x6'>('A4')
  const [orientation, setOrientation] = useState<'portrait' | 'landscape'>('portrait')
  const [docSides, setDocSides] = useState<'single' | 'double'>('single')
  const [copies, setCopies] = useState<number>(1)
  const [colorMode, setColorMode] = useState<'bw' | 'color'>('bw')
  const [docFilter, setDocFilter] = useState<'bw' | 'color' | 'original'>('bw')

  // Document mode state
  const [selectedDocFile, setSelectedDocFile] = useState<File | null>(null)
  const [docImageObj, setDocImageObj] = useState<HTMLImageElement | null>(null)
  const [docPreviewUrl, setDocPreviewUrl] = useState<string>('')

  // Card images state
  const [frontOriginal, setFrontOriginal] = useState<HTMLCanvasElement | HTMLImageElement | null>(null)
  const [frontProcessed, setFrontProcessed] = useState<HTMLImageElement | null>(null)
  const [frontRotation, setFrontRotation] = useState<number>(0)

  const [backOriginal, setBackOriginal] = useState<HTMLCanvasElement | HTMLImageElement | null>(null)
  const [backProcessed, setBackProcessed] = useState<HTMLImageElement | null>(null)
  const [backRotation, setBackRotation] = useState<number>(0)

  // Submitting & Success
  const [isSubmitting, setIsSubmitting] = useState(false)
  const [submitProgress, setSubmitProgress] = useState(0)
  const [successJob, setSuccessJob] = useState<{
    customer: string
    details: string
    shop: string
  } | null>(null)

  // Canvas Ref for A4 live preview
  const a4CanvasRef = useRef<HTMLCanvasElement>(null)
  const frontInputRef = useRef<HTMLInputElement>(null)
  const backInputRef = useRef<HTMLInputElement>(null)
  const docInputRef = useRef<HTMLInputElement>(null)

  // -------------------------------------------------------------
  // Process Card Image Helper
  // -------------------------------------------------------------
  const processCard = useCallback((
    orig: HTMLCanvasElement | HTMLImageElement | null,
    rotation: number,
    filter: 'bw' | 'color' | 'original',
    onDone: (img: HTMLImageElement) => void
  ) => {
    if (!orig) return
    const canvas = document.createElement('canvas')
    const is90 = rotation === 90 || rotation === 270
    const w = 'width' in orig ? orig.width : (orig as HTMLImageElement).naturalWidth
    const h = 'height' in orig ? orig.height : (orig as HTMLImageElement).naturalHeight

    canvas.width = is90 ? h : w
    canvas.height = is90 ? w : h
    const ctx = canvas.getContext('2d')
    if (!ctx) return

    ctx.translate(canvas.width / 2, canvas.height / 2)
    ctx.rotate((rotation * Math.PI) / 180)
    ctx.drawImage(orig, -w / 2, -h / 2)

    applyAdobeScanFilter(canvas, filter)

    const resImg = new Image()
    resImg.onload = () => onDone(resImg)
    resImg.src = canvas.toDataURL('image/jpeg', 0.95)
  }, [])

  // Re-process when filter changes
  useEffect(() => {
    if (frontOriginal) {
      processCard(frontOriginal, frontRotation, docFilter, setFrontProcessed)
    }
  }, [frontOriginal, frontRotation, docFilter, processCard])

  useEffect(() => {
    if (backOriginal) {
      processCard(backOriginal, backRotation, docFilter, setBackProcessed)
    }
  }, [backOriginal, backRotation, docFilter, processCard])

  // Helper for round rectangles on canvas
  const drawRoundRect = (
    ctx: CanvasRenderingContext2D,
    x: number, y: number, w: number, h: number, r: number,
    fill: boolean, stroke: boolean
  ) => {
    ctx.beginPath()
    ctx.moveTo(x + r, y)
    ctx.lineTo(x + w - r, y)
    ctx.quadraticCurveTo(x + w, y, x + w, y + r)
    ctx.lineTo(x + w, y + h - r)
    ctx.quadraticCurveTo(x + w, y + h, x + w - r, y + h)
    ctx.lineTo(x + r, y + h)
    ctx.quadraticCurveTo(x, y + h, x, y + h - r)
    ctx.lineTo(x, y + r)
    ctx.quadraticCurveTo(x, y, x + r, y)
    ctx.closePath()
    if (fill) ctx.fill()
    if (stroke) ctx.stroke()
  }

  // Draw Card Slot on A4 Canvas
  const drawCardSlot = useCallback((
    ctx: CanvasRenderingContext2D,
    img: HTMLImageElement | null,
    x: number, y: number, w: number, h: number,
    label: string
  ) => {
    ctx.save()
    if (!img) {
      ctx.strokeStyle = '#cbd5e1'
      ctx.lineWidth = 1.2
      ctx.fillStyle = '#f8fafc'
      drawRoundRect(ctx, x, y, w, h, 6, true, true)
      ctx.fillStyle = '#94a3b8'
      ctx.font = '11px sans-serif'
      ctx.textAlign = 'center'
      ctx.fillText(label, x + w / 2, y + h / 2 + 4)
    } else {
      drawRoundRect(ctx, x, y, w, h, 6, false, false)
      ctx.clip()
      ctx.drawImage(img, x, y, w, h)
      ctx.restore()

      ctx.save()
      ctx.strokeStyle = '#cbd5e1'
      ctx.lineWidth = 0.8
      drawRoundRect(ctx, x, y, w, h, 6, false, true)
    }
    ctx.restore()
  }, [])

  // Update A4 Sheet Canvas
  const updateA4Canvas = useCallback(() => {
    const canvas = a4CanvasRef.current
    if (!canvas) return
    const ctx = canvas.getContext('2d')
    if (!ctx) return
    const isLandscape = orientation === 'landscape'

    let baseW = 595, baseH = 842
    if (paperSize === 'Legal') {
      baseW = 595
      baseH = 1008
    } else if (paperSize === '4x6') {
      baseW = 400
      baseH = 600
    }
    canvas.width = isLandscape ? baseH : baseW
    canvas.height = isLandscape ? baseW : baseH

    const w = canvas.width
    const h = canvas.height

    ctx.fillStyle = '#ffffff'
    ctx.fillRect(0, 0, w, h)

    // Standard CR80 Size: 85.60 mm x 53.98 mm (~1.586 ratio)
    const refW = isLandscape ? baseH : baseW
    const cardW = Math.round(refW * (85.60 / 210.0))
    const cardH = Math.round(cardW / 1.586)
    const cardGap = Math.round(h * 0.03)

    if (cardLayout === 'both') {
      const totalCardsH = (cardH * 2) + cardGap
      const startY = placement === 'center' ? Math.max(10, Math.round((h - totalCardsH) / 2)) : Math.round(h * 0.045)
      const cardX = Math.round((w - cardW) / 2)

      drawCardSlot(ctx, frontProcessed, cardX, startY, cardW, cardH, t.frontCardPlaceholder)
      drawCardSlot(ctx, backProcessed, cardX, startY + cardH + cardGap, cardW, cardH, t.backCardPlaceholder)
    } else if (cardLayout === 'front_only') {
      const startY = placement === 'center' ? Math.max(10, Math.round((h - cardH) / 2)) : Math.round(h * 0.045)
      const cardX = Math.round((w - cardW) / 2)
      drawCardSlot(ctx, frontProcessed, cardX, startY, cardW, cardH, t.frontCardPlaceholder)
    } else if (cardLayout === 'back_only') {
      const startY = placement === 'center' ? Math.max(10, Math.round((h - cardH) / 2)) : Math.round(h * 0.045)
      const cardX = Math.round((w - cardW) / 2)
      drawCardSlot(ctx, backProcessed, cardX, startY, cardW, cardH, t.backCardPlaceholder)
    }

    // Faint dashed midline if top placement and portrait
    if (placement === 'top' && !isLandscape && cardLayout === 'both') {
      ctx.save()
      ctx.strokeStyle = '#f1f5f9'
      ctx.setLineDash([4, 4])
      ctx.beginPath()
      ctx.moveTo(20, h / 2)
      ctx.lineTo(w - 20, h / 2)
      ctx.stroke()
      ctx.restore()
    }
  }, [orientation, paperSize, cardLayout, placement, frontProcessed, backProcessed, drawCardSlot, t])

  useEffect(() => {
    if (mode === 'card') {
      updateA4Canvas()
    }
  }, [mode, updateA4Canvas])

  // -------------------------------------------------------------
  // Crop Modal State & Adobe Scan Algorithms
  // -------------------------------------------------------------
  const [cropModalOpen, setCropModalOpen] = useState(false)
  const [cropTarget, setCropTarget] = useState<'front' | 'back' | 'doc'>('front')
  const [cropImgSource, setCropImgSource] = useState<HTMLCanvasElement | HTMLImageElement | null>(null)
  const [cropCorners, setCropCorners] = useState<Point2D[]>([])
  const [activePinIndex, setActivePinIndex] = useState<number>(-1)
  const [activeEdgeIndex, setActiveEdgeIndex] = useState<number>(-1)

  const cropCanvasRef = useRef<HTMLCanvasElement>(null)
  const cropLoupeRef = useRef<HTMLDivElement>(null)
  const loupeCanvasRef = useRef<HTMLCanvasElement>(null)
  const dragStartRef = useRef<{ pos: Point2D; corners: Point2D[] } | null>(null)

  // Auto Corner Detection Logic (Adobe Scan Canny + Sobel + approxPolyDP)
  const runAutoDetect = useCallback((src: HTMLCanvasElement | HTMLImageElement) => {
    const srcW = 'width' in src ? src.width : (src as HTMLImageElement).naturalWidth
    const srcH = 'height' in src ? src.height : (src as HTMLImageElement).naturalHeight

    const maxDim = 420
    const scale = Math.min(1, maxDim / Math.max(srcW, srcH))
    const tw = Math.max(50, Math.round(srcW * scale))
    const th = Math.max(50, Math.round(srcH * scale))

    const aCanvas = document.createElement('canvas')
    aCanvas.width = tw
    aCanvas.height = th
    const aCtx = aCanvas.getContext('2d')
    if (!aCtx) return
    aCtx.drawImage(src, 0, 0, tw, th)
    const imgData = aCtx.getImageData(0, 0, tw, th)
    const d = imgData.data

    // 1. Grayscale
    const rawGray = new Uint8Array(tw * th)
    for (let i = 0, j = 0; i < d.length; i += 4, j++) {
      rawGray[j] = Math.round(0.299 * d[i] + 0.587 * d[i + 1] + 0.114 * d[i + 2])
    }

    // Separable 5x5 Gaussian Blur: [1, 4, 6, 4, 1] / 16
    const tempBlur = new Float32Array(tw * th)
    const blur = new Uint8Array(tw * th)
    const k5 = [1, 4, 6, 4, 1]

    for (let y = 0; y < th; y++) {
      const row = y * tw
      for (let x = 0; x < tw; x++) {
        let sum = 0, kw = 0
        for (let i = -2; i <= 2; i++) {
          const px = Math.min(tw - 1, Math.max(0, x + i))
          const w = k5[i + 2]
          sum += rawGray[row + px] * w
          kw += w
        }
        tempBlur[row + x] = sum / kw
      }
    }

    for (let y = 0; y < th; y++) {
      for (let x = 0; x < tw; x++) {
        let sum = 0, kw = 0
        for (let j = -2; j <= 2; j++) {
          const py = Math.min(th - 1, Math.max(0, y + j))
          const w = k5[j + 2]
          sum += tempBlur[py * tw + x] * w
          kw += w
        }
        blur[y * tw + x] = Math.round(sum / kw)
      }
    }

    // 2. Sobel Gradients
    const gxArr = new Int32Array(tw * th)
    const gyArr = new Int32Array(tw * th)
    const mag = new Float32Array(tw * th)
    let maxMag = 0

    for (let y = 1; y < th - 1; y++) {
      const row = y * tw
      const prev = (y - 1) * tw
      const next = (y + 1) * tw
      for (let x = 1; x < tw - 1; x++) {
        const gx = -blur[prev + x - 1] + blur[prev + x + 1]
                   - 2 * blur[row + x - 1] + 2 * blur[row + x + 1]
                   - blur[next + x - 1] + blur[next + x + 1]
        const gy = -blur[prev + x - 1] - 2 * blur[prev + x] - blur[prev + x + 1]
                   + blur[next + x - 1] + 2 * blur[next + x] + blur[next + x + 1]
        gxArr[row + x] = gx
        gyArr[row + x] = gy
        const m = Math.hypot(gx, gy)
        mag[row + x] = m
        if (m > maxMag) maxMag = m
      }
    }

    // Non-Maximum Suppression (NMS)
    const nms = new Float32Array(tw * th)
    for (let y = 1; y < th - 1; y++) {
      const row = y * tw
      for (let x = 1; x < tw - 1; x++) {
        const m = mag[row + x]
        if (m < 5) continue
        const gx = gxArr[row + x]
        const gy = gyArr[row + x]
        let angle = Math.atan2(gy, gx) * (180 / Math.PI)
        if (angle < 0) angle += 180

        let q0 = 0, q1 = 0
        if ((angle >= 0 && angle < 22.5) || (angle >= 157.5 && angle <= 180)) {
          q0 = mag[row + x + 1]
          q1 = mag[row + x - 1]
        } else if (angle >= 22.5 && angle < 67.5) {
          q0 = mag[(y + 1) * tw + x + 1]
          q1 = mag[(y - 1) * tw + x - 1]
        } else if (angle >= 67.5 && angle < 112.5) {
          q0 = mag[(y + 1) * tw + x]
          q1 = mag[(y - 1) * tw + x]
        } else if (angle >= 112.5 && angle < 157.5) {
          q0 = mag[(y + 1) * tw + x - 1]
          q1 = mag[(y - 1) * tw + x + 1]
        }

        if (m >= q0 && m >= q1) {
          nms[row + x] = m
        }
      }
    }

    // Canny Hysteresis Thresholding (75, 200)
    const lowThresh = Math.max(14, maxMag * (75 / 255))
    const highThresh = Math.max(35, maxMag * (200 / 255))
    const edges = new Uint8Array(tw * th)
    const edgeStack: number[] = []

    for (let y = 1; y < th - 1; y++) {
      const row = y * tw
      for (let x = 1; x < tw - 1; x++) {
        const val = nms[row + x]
        if (val >= highThresh) {
          edges[row + x] = 2
          edgeStack.push(row + x)
        } else if (val >= lowThresh) {
          edges[row + x] = 1
        }
      }
    }

    while (edgeStack.length > 0) {
      const idx = edgeStack.pop()!
      const cx = idx % tw
      const cy = Math.floor(idx / tw)
      for (let dy = -1; dy <= 1; dy++) {
        for (let dx = -1; dx <= 1; dx++) {
          if (dx === 0 && dy === 0) continue
          const nx = cx + dx, ny = cy + dy
          if (nx >= 0 && nx < tw && ny >= 0 && ny < th) {
            const nIdx = ny * tw + nx
            if (edges[nIdx] === 1) {
              edges[nIdx] = 2
              edgeStack.push(nIdx)
            }
          }
        }
      }
    }

    // 3. Morphological Closing (Dilate 5x5 + Erode 5x5)
    const binEdges = new Uint8Array(tw * th)
    for (let i = 0; i < edges.length; i++) {
      binEdges[i] = edges[i] === 2 ? 1 : 0
    }

    const dilated = new Uint8Array(tw * th)
    for (let y = 2; y < th - 2; y++) {
      for (let x = 2; x < tw - 2; x++) {
        let hit = 0
        for (let dy = -2; dy <= 2 && !hit; dy++) {
          for (let dx = -2; dx <= 2; dx++) {
            if (binEdges[(y + dy) * tw + (x + dx)] === 1) { hit = 1; break }
          }
        }
        dilated[y * tw + x] = hit
      }
    }

    const closed = new Uint8Array(tw * th)
    for (let y = 2; y < th - 2; y++) {
      for (let x = 2; x < tw - 2; x++) {
        let allOn = 1
        for (let dy = -2; dy <= 2 && allOn; dy++) {
          for (let dx = -2; dx <= 2; dx++) {
            if (dilated[(y + dy) * tw + (x + dx)] === 0) { allOn = 0; break }
          }
        }
        closed[y * tw + x] = allOn
      }
    }

    // 4. Contour Boundary Extraction
    const visited = new Uint8Array(tw * th)
    const candidateContours: Point2D[][] = []
    const minContourLen = Math.round((tw + th) * 0.22)

    for (let y = 2; y < th - 2; y++) {
      for (let x = 2; x < tw - 2; x++) {
        const idx = y * tw + x
        if (closed[idx] === 1 && !visited[idx]) {
          const pts: Point2D[] = []
          const q = [idx]
          visited[idx] = 1
          while (q.length > 0) {
            const curr = q.pop()!
            const cx = curr % tw
            const cy = Math.floor(curr / tw)
            pts.push({ x: cx, y: cy })

            for (let dy = -1; dy <= 1; dy++) {
              for (let dx = -1; dx <= 1; dx++) {
                if (dx === 0 && dy === 0) continue
                const nx = cx + dx, ny = cy + dy
                if (nx >= 0 && nx < tw && ny >= 0 && ny < th) {
                  const nIdx = ny * tw + nx
                  if (closed[nIdx] === 1 && !visited[nIdx]) {
                    visited[nIdx] = 1
                    q.push(nIdx)
                  }
                }
              }
            }
          }
          if (pts.length >= minContourLen) {
            candidateContours.push(pts)
          }
        }
      }
    }

    candidateContours.sort((a, b) => b.length - a.length)

    // Convex Hull
    function getConvexHull(points: Point2D[]): Point2D[] {
      if (points.length <= 3) return points.slice()
      const sorted = points.slice().sort((a, b) => a.x === b.x ? a.y - b.y : a.x - b.x)
      const cross = (o: Point2D, a: Point2D, b: Point2D) => (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
      const lower: Point2D[] = []
      for (const p of sorted) {
        while (lower.length >= 2 && cross(lower[lower.length - 2], lower[lower.length - 1], p) <= 0) lower.pop()
        lower.push(p)
      }
      const upper: Point2D[] = []
      for (let i = sorted.length - 1; i >= 0; i--) {
        const p = sorted[i]
        while (upper.length >= 2 && cross(upper[upper.length - 2], upper[upper.length - 1], p) <= 0) upper.pop()
        upper.push(p)
      }
      lower.pop()
      upper.pop()
      return lower.concat(upper)
    }

    // RDP approxPolyDP
    function approxPolyDP(pts: Point2D[], eps: number): Point2D[] {
      if (pts.length <= 2) return pts.slice()
      let maxD = 0, maxIdx = 0
      const p1 = pts[0], p2 = pts[pts.length - 1]
      const dx = p2.x - p1.x, dy = p2.y - p1.y
      const len = Math.hypot(dx, dy)

      for (let i = 1; i < pts.length - 1; i++) {
        const d = len < 1e-5 ? Math.hypot(pts[i].x - p1.x, pts[i].y - p1.y) : Math.abs((pts[i].x - p1.x) * dy - (pts[i].y - p1.y) * dx) / len
        if (d > maxD) { maxD = d; maxIdx = i }
      }

      if (maxD > eps) {
        const r1 = approxPolyDP(pts.slice(0, maxIdx + 1), eps)
        const r2 = approxPolyDP(pts.slice(maxIdx), eps)
        return r1.slice(0, -1).concat(r2)
      }
      return [p1, p2]
    }

    let detected: Point2D[] | null = null
    const minArea = tw * th * 0.10

    for (const cnt of candidateContours) {
      const hull = getConvexHull(cnt)
      if (hull.length < 4) continue

      let perim = 0
      for (let i = 0; i < hull.length; i++) {
        const p1 = hull[i], p2 = hull[(i + 1) % hull.length]
        perim += Math.hypot(p2.x - p1.x, p2.y - p1.y)
      }

      for (let epsRatio = 0.015; epsRatio <= 0.055; epsRatio += 0.005) {
        const closedHull = hull.concat([hull[0]])
        const approx = approxPolyDP(closedHull, perim * epsRatio)
        if (approx.length > 1 && approx[0].x === approx[approx.length - 1].x && approx[0].y === approx[approx.length - 1].y) {
          approx.pop()
        }

        if (approx.length === 4) {
          let a = 0
          for (let i = 0; i < approx.length; i++) {
            const j = (i + 1) % approx.length
            a += approx[i].x * approx[j].y - approx[j].x * approx[i].y
          }
          const area = Math.abs(a) / 2

          if (area >= minArea) {
            let tl = approx[0], br = approx[0], tr = approx[0], bl = approx[0]
            let minSum = Infinity, maxSum = -Infinity
            let minDiff = Infinity, maxDiff = -Infinity
            for (const p of approx) {
              const sum = p.x + p.y
              const diff = p.y - p.x
              if (sum < minSum) { minSum = sum; tl = p }
              if (sum > maxSum) { maxSum = sum; br = p }
              if (diff < minDiff) { minDiff = diff; tr = p }
              if (diff > maxDiff) { maxDiff = diff; bl = p }
            }

            const topW = Math.hypot(tr.x - tl.x, tr.y - tl.y)
            const botW = Math.hypot(br.x - bl.x, br.y - bl.y)
            const leftH = Math.hypot(bl.x - tl.x, bl.y - tl.y)
            const rightH = Math.hypot(br.x - tr.x, br.y - tr.y)

            if (topW > tw * 0.20 && botW > tw * 0.20 && leftH > th * 0.15 && rightH > th * 0.15) {
              const inv = 1 / scale
              detected = [
                { x: Math.max(0, Math.min(srcW, Math.round(tl.x * inv))), y: Math.max(0, Math.min(srcH, Math.round(tl.y * inv))) },
                { x: Math.max(0, Math.min(srcW, Math.round(tr.x * inv))), y: Math.max(0, Math.min(srcH, Math.round(tr.y * inv))) },
                { x: Math.max(0, Math.min(srcW, Math.round(br.x * inv))), y: Math.max(0, Math.min(srcH, Math.round(br.y * inv))) },
                { x: Math.max(0, Math.min(srcW, Math.round(bl.x * inv))), y: Math.max(0, Math.min(srcH, Math.round(bl.y * inv))) }
              ]
              break
            }
          }
        }
      }
      if (detected) break
    }

    if (detected) {
      setCropCorners(detected)
    } else {
      // Fallback: standard CR80 centered quad with 8% margin
      const marginX = Math.round(srcW * 0.08)
      const cardW = srcW - 2 * marginX
      const cardH = Math.round(cardW / 1.58577)
      const startY = Math.max(0, Math.round((srcH - cardH) / 2))
      setCropCorners([
        { x: marginX, y: startY },
        { x: marginX + cardW, y: startY },
        { x: marginX + cardW, y: Math.min(srcH, startY + cardH) },
        { x: marginX, y: Math.min(srcH, startY + cardH) }
      ])
    }
  }, [])

  // Open Crop Modal
  const openCropModal = (target: 'front' | 'back' | 'doc', e?: React.MouseEvent) => {
    if (e) {
      e.preventDefault()
      e.stopPropagation()
    }
    setCropTarget(target)
    let src: HTMLCanvasElement | HTMLImageElement | null = null
    if (target === 'front') src = frontOriginal
    else if (target === 'back') src = backOriginal
    else if (target === 'doc') src = docImageObj

    if (!src) return
    setCropImgSource(src)
    setCropModalOpen(true)
    runAutoDetect(src)
  }

  // Draw Crop Canvas
  const drawCropCanvas = useCallback(() => {
    const canvas = cropCanvasRef.current
    if (!canvas || !cropImgSource || cropCorners.length < 4) return
    const ctx = canvas.getContext('2d')
    if (!ctx) return

    const srcW = 'width' in cropImgSource ? cropImgSource.width : (cropImgSource as HTMLImageElement).naturalWidth
    const srcH = 'height' in cropImgSource ? cropImgSource.height : (cropImgSource as HTMLImageElement).naturalHeight
    canvas.width = srcW
    canvas.height = srcH

    ctx.clearRect(0, 0, canvas.width, canvas.height)
    ctx.drawImage(cropImgSource, 0, 0)

    // Darken outside quadrilateral
    ctx.save()
    ctx.fillStyle = 'rgba(0, 0, 0, 0.55)'
    ctx.fillRect(0, 0, canvas.width, canvas.height)

    // Cut out quad
    ctx.globalCompositeOperation = 'destination-out'
    ctx.beginPath()
    ctx.moveTo(cropCorners[0].x, cropCorners[0].y)
    ctx.lineTo(cropCorners[1].x, cropCorners[1].y)
    ctx.lineTo(cropCorners[2].x, cropCorners[2].y)
    ctx.lineTo(cropCorners[3].x, cropCorners[3].y)
    ctx.closePath()
    ctx.fill()
    ctx.restore()

    // Faint Rule of Thirds grid
    ctx.save()
    ctx.strokeStyle = 'rgba(255, 255, 255, 0.22)'
    ctx.lineWidth = 1.5
    const [tl, tr, br, bl] = cropCorners
    for (let tStep = 1; tStep <= 2; tStep++) {
      const frac = tStep / 3
      const lx = tl.x + (bl.x - tl.x) * frac
      const ly = tl.y + (bl.y - tl.y) * frac
      const rx = tr.x + (br.x - tr.x) * frac
      const ry = tr.y + (br.y - tr.y) * frac
      ctx.beginPath()
      ctx.moveTo(lx, ly)
      ctx.lineTo(rx, ry)
      ctx.stroke()

      const tx = tl.x + (tr.x - tl.x) * frac
      const ty = tl.y + (tr.y - tl.y) * frac
      const bx = bl.x + (br.x - bl.x) * frac
      const by = bl.y + (br.y - bl.y) * frac
      ctx.beginPath()
      ctx.moveTo(tx, ty)
      ctx.lineTo(bx, by)
      ctx.stroke()
    }
    ctx.restore()

    // Quad Outline
    ctx.save()
    ctx.strokeStyle = '#2563eb'
    ctx.lineWidth = Math.max(3.5, Math.round(canvas.width / 220))
    ctx.beginPath()
    ctx.moveTo(cropCorners[0].x, cropCorners[0].y)
    ctx.lineTo(cropCorners[1].x, cropCorners[1].y)
    ctx.lineTo(cropCorners[2].x, cropCorners[2].y)
    ctx.lineTo(cropCorners[3].x, cropCorners[3].y)
    ctx.closePath()
    ctx.stroke()

    // 4 Edge Midpoint Handles
    const edgeRadius = Math.max(7, Math.round(canvas.width / 110))
    for (let eIdx = 0; eIdx < 4; eIdx++) {
      const p1 = cropCorners[eIdx]
      const p2 = cropCorners[(eIdx + 1) % 4]
      const mid = { x: (p1.x + p2.x) / 2, y: (p1.y + p2.y) / 2 }
      ctx.beginPath()
      ctx.arc(mid.x, mid.y, edgeRadius, 0, Math.PI * 2)
      ctx.fillStyle = eIdx === activeEdgeIndex ? '#00f2fe' : '#ffffff'
      ctx.fill()
      ctx.lineWidth = 2.5
      ctx.strokeStyle = '#2563eb'
      ctx.stroke()
    }

    // 4 Corner Pin Handles
    const pinRadius = Math.max(15, Math.round(canvas.width / 50))
    cropCorners.forEach((pt, idx) => {
      ctx.beginPath()
      ctx.arc(pt.x, pt.y, pinRadius, 0, Math.PI * 2)
      ctx.fillStyle = '#ffffff'
      ctx.fill()
      ctx.lineWidth = 4
      ctx.strokeStyle = idx === activePinIndex ? '#00f2fe' : '#2563eb'
      ctx.stroke()

      ctx.beginPath()
      ctx.arc(pt.x, pt.y, pinRadius * 0.35, 0, Math.PI * 2)
      ctx.fillStyle = idx === activePinIndex ? '#00f2fe' : '#2563eb'
      ctx.fill()
    })
    ctx.restore()
  }, [cropImgSource, cropCorners, activePinIndex, activeEdgeIndex])

  useEffect(() => {
    if (cropModalOpen) {
      drawCropCanvas()
    }
  }, [cropModalOpen, drawCropCanvas])

  // Magnifying Loupe Update
  const updateLoupe = useCallback((screenX: number, screenY: number, cornerX: number, cornerY: number) => {
    const loupe = cropLoupeRef.current
    const lCanvas = loupeCanvasRef.current
    if (!loupe || !lCanvas || !cropImgSource) return
    loupe.style.display = 'block'

    const loupeRadius = 55
    const loupeTop = Math.max(10, screenY - 140)
    const loupeLeft = Math.max(10, Math.min(window.innerWidth - 120, screenX - loupeRadius))
    loupe.style.top = `${loupeTop}px`
    loupe.style.left = `${loupeLeft}px`

    const lCtx = lCanvas.getContext('2d')
    if (!lCtx) return
    const zoom = 2.5
    const lw = lCanvas.width
    const lh = lCanvas.height
    lCtx.clearRect(0, 0, lw, lh)

    const srcSize = lw / zoom
    const sx = cornerX - srcSize / 2
    const sy = cornerY - srcSize / 2

    lCtx.drawImage(
      cropImgSource,
      sx, sy, srcSize, srcSize,
      0, 0, lw, lh
    )
  }, [cropImgSource])

  const hideLoupe = useCallback(() => {
    if (cropLoupeRef.current) {
      cropLoupeRef.current.style.display = 'none'
    }
  }, [])

  // Corner Snap to Edge on pointer up
  const snapCornerToEdge = useCallback((cornerIdx: number, corners: Point2D[]) => {
    if (!cropImgSource) return
    const pt = corners[cornerIdx]
    const searchRadius = 24
    const srcW = 'width' in cropImgSource ? cropImgSource.width : (cropImgSource as HTMLImageElement).naturalWidth
    const srcH = 'height' in cropImgSource ? cropImgSource.height : (cropImgSource as HTMLImageElement).naturalHeight

    const minX = Math.max(2, Math.round(pt.x - searchRadius))
    const maxX = Math.min(srcW - 3, Math.round(pt.x + searchRadius))
    const minY = Math.max(2, Math.round(pt.y - searchRadius))
    const maxY = Math.min(srcH - 3, Math.round(pt.y + searchRadius))
    const w = maxX - minX + 1
    const h = maxY - minY + 1
    if (w <= 4 || h <= 4) return

    const pCanvas = document.createElement('canvas')
    pCanvas.width = w
    pCanvas.height = h
    const pCtx = pCanvas.getContext('2d')
    if (!pCtx) return
    pCtx.drawImage(cropImgSource, minX, minY, w, h, 0, 0, w, h)
    const pData = pCtx.getImageData(0, 0, w, h).data

    let bestX = pt.x
    let bestY = pt.y
    let maxG = 0

    for (let y = 1; y < h - 1; y++) {
      for (let x = 1; x < w - 1; x++) {
        const getL = (px: number, py: number) => {
          const i = (py * w + px) * 4
          return 0.299 * pData[i] + 0.587 * pData[i + 1] + 0.114 * pData[i + 2]
        }
        const gx = -getL(x - 1, y) + getL(x + 1, y)
        const gy = -getL(x, y - 1) + getL(x, y + 1)
        const g = Math.hypot(gx, gy)
        if (g > maxG) {
          maxG = g
          bestX = minX + x
          bestY = minY + y
        }
      }
    }

    if (maxG > 35 && Math.hypot(bestX - pt.x, bestY - pt.y) <= searchRadius) {
      corners[cornerIdx].x = bestX
      corners[cornerIdx].y = bestY
    }
  }, [cropImgSource])

  // Pointer drag interactions
  const handlePointerDown = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const canvas = cropCanvasRef.current
    if (!canvas || cropCorners.length < 4) return
    canvas.setPointerCapture(e.pointerId)

    const rect = canvas.getBoundingClientRect()
    const scaleX = rect.width / canvas.width
    const scaleY = rect.height / canvas.height
    const clientX = e.clientX
    const clientY = e.clientY

    let foundPin = -1
    let minCornerDist = Infinity
    const cornerTouchRadius = 44

    cropCorners.forEach((pt, idx) => {
      const screenX = rect.left + pt.x * scaleX
      const screenY = rect.top + pt.y * scaleY
      const dist = Math.hypot(screenX - clientX, screenY - clientY)
      if (dist < cornerTouchRadius && dist < minCornerDist) {
        minCornerDist = dist
        foundPin = idx
      }
    })

    let foundEdge = -1
    if (foundPin === -1) {
      const edgeTouchRadius = 38
      let minEdgeDist = Infinity
      for (let eIdx = 0; eIdx < 4; eIdx++) {
        const p1 = cropCorners[eIdx]
        const p2 = cropCorners[(eIdx + 1) % 4]
        const mid = { x: (p1.x + p2.x) / 2, y: (p1.y + p2.y) / 2 }
        const screenX = rect.left + mid.x * scaleX
        const screenY = rect.top + mid.y * scaleY
        const dist = Math.hypot(screenX - clientX, screenY - clientY)
        if (dist < edgeTouchRadius && dist < minEdgeDist) {
          minEdgeDist = dist
          foundEdge = eIdx
        }
      }
    }

    setActivePinIndex(foundPin)
    setActiveEdgeIndex(foundEdge)

    const canvasX = (clientX - rect.left) * (canvas.width / rect.width)
    const canvasY = (clientY - rect.top) * (canvas.height / rect.height)

    if (foundPin !== -1) {
      updateLoupe(clientX, clientY, cropCorners[foundPin].x, cropCorners[foundPin].y)
    } else if (foundEdge !== -1) {
      dragStartRef.current = {
        pos: { x: canvasX, y: canvasY },
        corners: cropCorners.map(p => ({ x: p.x, y: p.y }))
      }
    }
  }

  const handlePointerMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const canvas = cropCanvasRef.current
    if (!canvas) return
    if (activePinIndex === -1 && activeEdgeIndex === -1) return

    const rect = canvas.getBoundingClientRect()
    const clientX = e.clientX
    const clientY = e.clientY
    const canvasX = Math.max(0, Math.min(canvas.width, (clientX - rect.left) * (canvas.width / rect.width)))
    const canvasY = Math.max(0, Math.min(canvas.height, (clientY - rect.top) * (canvas.height / rect.height)))

    if (activePinIndex !== -1) {
      setCropCorners(prev => {
        const next = [...prev]
        next[activePinIndex] = { x: canvasX, y: canvasY }
        return next
      })
      updateLoupe(clientX, clientY, canvasX, canvasY)
    } else if (activeEdgeIndex !== -1 && dragStartRef.current) {
      const dx = canvasX - dragStartRef.current.pos.x
      const dy = canvasY - dragStartRef.current.pos.y
      const startCorners = dragStartRef.current.corners
      const idx1 = activeEdgeIndex
      const idx2 = (activeEdgeIndex + 1) % 4

      setCropCorners(prev => {
        const next = [...prev]
        next[idx1] = {
          x: Math.max(0, Math.min(canvas.width, startCorners[idx1].x + dx)),
          y: Math.max(0, Math.min(canvas.height, startCorners[idx1].y + dy))
        }
        next[idx2] = {
          x: Math.max(0, Math.min(canvas.width, startCorners[idx2].x + dx)),
          y: Math.max(0, Math.min(canvas.height, startCorners[idx2].y + dy))
        }
        return next
      })
    }
  }

  const handlePointerUp = () => {
    if (activePinIndex !== -1) {
      setCropCorners(prev => {
        const next = [...prev]
        snapCornerToEdge(activePinIndex, next)
        return next
      })
      setActivePinIndex(-1)
    }
    setActiveEdgeIndex(-1)
    dragStartRef.current = null
    hideLoupe()
  }

  // Apply Crop and Warp
  const applyCropAndClose = () => {
    if (!cropImgSource || cropCorners.length < 4) return
    const [tl, tr, br, bl] = cropCorners
    const topW = Math.hypot(tr.x - tl.x, tr.y - tl.y)
    const botW = Math.hypot(br.x - bl.x, br.y - bl.y)
    const leftH = Math.hypot(bl.x - tl.x, bl.y - tl.y)
    const rightH = Math.hypot(br.x - tr.x, br.y - tr.y)

    let targetW = Math.round(Math.max(topW, botW))
    let targetH = Math.round(Math.max(leftH, rightH))

    if (cropTarget === 'front' || cropTarget === 'back') {
      // Standard ISO/IEC CR80 aspect ratio (85.60mm / 53.98mm = 1.58577)
      targetH = Math.round(targetW / 1.58577)
    }

    const srcW = 'width' in cropImgSource ? cropImgSource.width : (cropImgSource as HTMLImageElement).naturalWidth
    const srcH = 'height' in cropImgSource ? cropImgSource.height : (cropImgSource as HTMLImageElement).naturalHeight

    const warpedCanvas = warpPerspective(cropImgSource, srcW, srcH, { tl, tr, br, bl }, targetW, targetH)

    const croppedImg = new Image()
    croppedImg.onload = () => {
      if (cropTarget === 'front') {
        setFrontOriginal(croppedImg)
        processCard(croppedImg, frontRotation, docFilter, setFrontProcessed)
      } else if (cropTarget === 'back') {
        setBackOriginal(croppedImg)
        processCard(croppedImg, backRotation, docFilter, setBackProcessed)
      } else if (cropTarget === 'doc') {
        setDocImageObj(croppedImg)
        setDocPreviewUrl(croppedImg.src)
      }
      setCropModalOpen(false)
      hideLoupe()
    }
    croppedImg.src = warpedCanvas.toDataURL('image/jpeg', 0.95)
  }

  // Card file loader
  const handleCardFile = (e: React.ChangeEvent<HTMLInputElement>, side: 'front' | 'back') => {
    const file = e.target.files && e.target.files[0]
    if (!file) return
    const reader = new FileReader()
    reader.onload = ev => {
      const img = new Image()
      img.onload = () => {
        const maxD = 1800
        let finalCanvasOrImg: HTMLCanvasElement | HTMLImageElement = img
        if (img.width > maxD || img.height > maxD) {
          const s = maxD / Math.max(img.width, img.height)
          const dc = document.createElement('canvas')
          dc.width = Math.round(img.width * s)
          dc.height = Math.round(img.height * s)
          dc.getContext('2d')?.drawImage(img, 0, 0, dc.width, dc.height)
          finalCanvasOrImg = dc
        }

        if (side === 'front') {
          setFrontOriginal(finalCanvasOrImg)
          setFrontRotation(0)
          processCard(finalCanvasOrImg, 0, docFilter, setFrontProcessed)
        } else {
          setBackOriginal(finalCanvasOrImg)
          setBackRotation(0)
          processCard(finalCanvasOrImg, 0, docFilter, setBackProcessed)
        }

        // Auto launch crop modal
        setCropTarget(side)
        setCropImgSource(finalCanvasOrImg)
        setCropModalOpen(true)
        runAutoDetect(finalCanvasOrImg)
      }
      img.src = ev.target?.result as string
    }
    reader.readAsDataURL(file)
    e.target.value = ''
  }

  // Document file loader
  const handleDocFile = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files && e.target.files[0]
    if (!file) return
    setSelectedDocFile(file)

    if (file.type.startsWith('image/')) {
      const reader = new FileReader()
      reader.onload = ev => {
        const img = new Image()
        img.onload = () => {
          setDocImageObj(img)
          setDocPreviewUrl(img.src)
        }
        img.src = ev.target?.result as string
      }
      reader.readAsDataURL(file)
    } else {
      setDocImageObj(null)
      setDocPreviewUrl('')
    }
    e.target.value = ''
  }

  // -------------------------------------------------------------
  // Generate 300 DPI High-Resolution A4 Sheet Blob for Printing
  // -------------------------------------------------------------
  const generateHighResA4Blob = async (): Promise<Blob> => {
    const printCanvas = document.createElement('canvas')
    const isLandscape = orientation === 'landscape'

    let printW = 2480, printH = 3508
    if (paperSize === 'Legal') {
      printW = 2480
      printH = 4200
    } else if (paperSize === '4x6') {
      printW = 1200
      printH = 1800
    }

    printCanvas.width = isLandscape ? printH : printW
    printCanvas.height = isLandscape ? printW : printH
    const ctx = printCanvas.getContext('2d')!
    const w = printCanvas.width
    const h = printCanvas.height

    ctx.fillStyle = '#ffffff'
    ctx.fillRect(0, 0, w, h)

    const refW = isLandscape ? printH : printW
    const cardW = Math.round(refW * (85.60 / 210.0))
    const cardH = Math.round(cardW / 1.586)
    const cardGap = Math.round(h * 0.03)

    if (cardLayout === 'both') {
      const totalCardsH = (cardH * 2) + cardGap
      const startY = placement === 'center' ? Math.max(20, Math.round((h - totalCardsH) / 2)) : Math.round(h * 0.045)
      const cardX = Math.round((w - cardW) / 2)

      if (frontProcessed) ctx.drawImage(frontProcessed, cardX, startY, cardW, cardH)
      if (backProcessed) ctx.drawImage(backProcessed, cardX, startY + cardH + cardGap, cardW, cardH)
    } else if (cardLayout === 'front_only') {
      const startY = placement === 'center' ? Math.max(20, Math.round((h - cardH) / 2)) : Math.round(h * 0.045)
      const cardX = Math.round((w - cardW) / 2)
      if (frontProcessed) ctx.drawImage(frontProcessed, cardX, startY, cardW, cardH)
    } else if (cardLayout === 'back_only') {
      const startY = placement === 'center' ? Math.max(20, Math.round((h - cardH) / 2)) : Math.round(h * 0.045)
      const cardX = Math.round((w - cardW) / 2)
      if (backProcessed) ctx.drawImage(backProcessed, cardX, startY, cardW, cardH)
    }

    return new Promise((resolve) => {
      printCanvas.toBlob(blob => resolve(blob!), 'image/jpeg', 0.96)
    })
  }

  // -------------------------------------------------------------
  // Submit Print Job directly to Supabase Cloud
  // -------------------------------------------------------------
  const handleSubmit = async () => {
    const trimmedName = customerName.trim()
    if (!trimmedName) {
      alert(t.alertEnterName)
      return
    }

    let payloadBlob: Blob | File
    let fileName: string
    const isCard = mode === 'card'

    if (isCard) {
      if (!frontOriginal && !backOriginal) {
        alert(t.alertUploadCard)
        return
      }
      setIsSubmitting(true)
      setSubmitProgress(20)
      payloadBlob = await generateHighResA4Blob()
      fileName = `${trimmedName.replace(/[^a-zA-Z0-9]/g, '_')}_IDCard_A4.jpg`
    } else {
      if (!selectedDocFile) {
        alert(t.alertSelectDoc)
        return
      }
      setIsSubmitting(true)
      setSubmitProgress(20)
      payloadBlob = selectedDocFile
      fileName = selectedDocFile.name
    }

    try {
      setSubmitProgress(45)
      const cleanFileName = fileName.replace(/[^a-zA-Z0-9._-]/g, '_')
      const timestamp = Date.now()
      const storagePath = `${shopId}/${timestamp}_${cleanFileName}`

      // 1. Upload to Supabase Storage 'temp_docs' bucket
      const { error: uploadError } = await supabase.storage
        .from('temp_docs')
        .upload(storagePath, payloadBlob, {
          cacheControl: '3600',
          upsert: true,
          contentType: payloadBlob.type || 'image/jpeg',
        })

      if (uploadError) {
        console.error('Storage upload error:', uploadError)
        throw new Error(uploadError.message || 'File upload failed')
      }

      setSubmitProgress(75)

      // 2. Get Public Storage URL
      const { data: publicUrlData } = supabase.storage
        .from('temp_docs')
        .getPublicUrl(storagePath)

      const fileUrl = publicUrlData.publicUrl

      setSubmitProgress(90)

      // 3. Format customer name with metadata for desktop queue
      const printDetailsStr = `${copies} ${copies > 1 ? 'Copies' : 'Copy'} · ${colorMode === 'bw' ? 'B&W' : 'Color'} · ${paperSize} (${orientation})${isCard ? ' · CR80 ' + cardLayout : ' · ' + (docSides === 'single' ? '1-Sided' : 'Duplex')}`
      const formattedCustomer = `${trimmedName} [${printDetailsStr}]`

      // 4. Insert row into print_jobs table
      const { error: jobError } = await supabase
        .from('print_jobs')
        .insert([
          {
            shop_unique_id: shopId,
            customer_name: formattedCustomer,
            file_url: fileUrl,
            status: 'pending',
          }
        ])

      if (jobError) {
        console.error('Database insert error:', jobError)
        throw new Error(jobError.message || 'Failed to submit print job')
      }

      setSubmitProgress(100)
      setSuccessJob({
        customer: trimmedName,
        details: printDetailsStr,
        shop: shopDetails?.shop_name || shopId
      })
      window.scrollTo({ top: 0, behavior: 'smooth' })
    } catch (err: unknown) {
      alert(`${t.alertUploadFail} (${err instanceof Error ? err.message : String(err)})`)
    } finally {
      setIsSubmitting(false)
    }
  }

  const resetForm = () => {
    setSelectedDocFile(null)
    setDocImageObj(null)
    setDocPreviewUrl('')
    setFrontOriginal(null)
    setFrontProcessed(null)
    setBackOriginal(null)
    setBackProcessed(null)
    setSuccessJob(null)
    setSubmitProgress(0)
  }

  return (
    <div className="min-h-screen bg-[#090d16] text-[#f8fafc] font-sans antialiased pb-12 selection:bg-blue-600 selection:text-white">
      {/* Top Header */}
      <header className="bg-[#131b2e] border-b border-[#1e293b] sticky top-0 z-30 px-4 py-3 flex items-center justify-between shadow-sm">
        <div className="flex items-center gap-2.5">
          <div className="w-8 h-8 rounded-lg bg-blue-600 text-white flex items-center justify-center font-bold text-base shadow-sm">
            🖨️
          </div>
          <div>
            <div className="font-bold text-sm leading-tight text-white flex items-center gap-1.5">
              <span>{t.portalTitle}</span>
            </div>
            {shopDetails && (
              <div className="text-[11px] text-blue-400 font-medium leading-none mt-0.5">
                {shopDetails.shop_name}
              </div>
            )}
          </div>
        </div>

        <div className="flex items-center gap-2">
          {/* Language Switcher */}
          <button
            type="button"
            onClick={toggleLanguage}
            className="bg-[#090d16] border border-[#1e293b] text-[#f8fafc] hover:border-blue-500 px-2.5 py-1 rounded-full text-xs font-semibold flex items-center gap-1 transition-colors"
          >
            🌐 <span>{t.langLabel}</span>
          </button>

          {/* Cloud Online Badge (Green dot requested) */}
          <div className="bg-emerald-500/15 border border-emerald-500/30 text-emerald-400 text-xs font-bold px-2.5 py-1 rounded-full flex items-center gap-1.5 shadow-sm">
            <span className="w-2 h-2 rounded-full bg-emerald-400 animate-pulse" />
            <span>{t.cloudBadge}</span>
          </div>
        </div>
      </header>

      {/* Main Container */}
      <div className="max-w-[520px] mx-auto px-4 mt-4">
        {!successJob ? (
          <div className="bg-[#131b2e] border border-[#1e293b] rounded-2xl p-4 sm:p-5 shadow-xl">
            {/* Customer Name */}
            <div className="mb-4">
              <label className="block text-xs font-semibold text-slate-200 mb-1.5">
                {t.customerNameLabel}{' '}
                <span className="text-slate-400 font-normal">{t.customerNameHint}</span>
              </label>
              <input
                type="text"
                value={customerName}
                onChange={e => setCustomerName(e.target.value)}
                placeholder={t.customerPlaceholder}
                className="w-full px-3.5 py-2.5 bg-transparent border border-[#1e293b] focus:border-blue-500 focus:ring-2 focus:ring-blue-500/20 rounded-xl text-sm text-white placeholder-slate-500 outline-none transition-all"
                required
              />
            </div>

            {/* Mode Tabs */}
            <div className="grid grid-cols-2 gap-1.5 bg-[#090d16] p-1 rounded-xl border border-[#1e293b] mb-4">
              <button
                type="button"
                onClick={() => setMode('card')}
                className={`py-2 px-3 rounded-lg text-xs font-semibold flex items-center justify-center gap-1.5 transition-all ${
                  mode === 'card'
                    ? 'bg-[#131b2e] text-blue-400 shadow-sm'
                    : 'text-slate-400 hover:text-slate-200'
                }`}
              >
                <span>🪪</span>
                <span>{t.tabCard}</span>
              </button>
              <button
                type="button"
                onClick={() => setMode('doc')}
                className={`py-2 px-3 rounded-lg text-xs font-semibold flex items-center justify-center gap-1.5 transition-all ${
                  mode === 'doc'
                    ? 'bg-[#131b2e] text-blue-400 shadow-sm'
                    : 'text-slate-400 hover:text-slate-200'
                }`}
              >
                <span>📄</span>
                <span>{t.tabDoc}</span>
              </button>
            </div>

            {/* Card Mode Section */}
            {mode === 'card' && (
              <div>
                {/* Card Print Layout */}
                <div className="mb-4">
                  <label className="block text-xs font-semibold text-slate-200 mb-1.5">
                    {t.cardLayoutLabel}
                  </label>
                  <div className="grid grid-cols-3 gap-1.5">
                    <button
                      type="button"
                      onClick={() => setCardLayout('both')}
                      className={`py-2 px-2 rounded-lg text-xs font-semibold border transition-all text-center ${
                        cardLayout === 'both'
                          ? 'border-blue-500 bg-blue-500/10 text-blue-400'
                          : 'border-[#1e293b] bg-[#090d16] text-slate-400 hover:border-slate-700'
                      }`}
                    >
                      🪪 {t.layoutBoth}
                    </button>
                    <button
                      type="button"
                      onClick={() => setCardLayout('front_only')}
                      className={`py-2 px-2 rounded-lg text-xs font-semibold border transition-all text-center ${
                        cardLayout === 'front_only'
                          ? 'border-blue-500 bg-blue-500/10 text-blue-400'
                          : 'border-[#1e293b] bg-[#090d16] text-slate-400 hover:border-slate-700'
                      }`}
                    >
                      📄 {t.layoutFront}
                    </button>
                    <button
                      type="button"
                      onClick={() => setCardLayout('back_only')}
                      className={`py-2 px-2 rounded-lg text-xs font-semibold border transition-all text-center ${
                        cardLayout === 'back_only'
                          ? 'border-blue-500 bg-blue-500/10 text-blue-400'
                          : 'border-[#1e293b] bg-[#090d16] text-slate-400 hover:border-slate-700'
                      }`}
                    >
                      📑 {t.layoutBack}
                    </button>
                  </div>
                </div>

                {/* Card Upload Grid */}
                <div className={`grid gap-3 mb-4 ${cardLayout === 'both' ? 'grid-cols-2' : 'grid-cols-1'}`}>
                  {/* Front Card */}
                  {(cardLayout === 'both' || cardLayout === 'front_only') && (
                    <div
                      onClick={() => {
                        if (!frontProcessed && frontInputRef.current) {
                          frontInputRef.current.click()
                        }
                      }}
                      className="border-2 border-dashed border-[#1e293b] hover:border-blue-500 rounded-xl aspect-[1.586] flex flex-col items-center justify-center relative bg-[#090d16] overflow-hidden cursor-pointer transition-colors"
                    >
                      <input
                        ref={frontInputRef}
                        type="file"
                        accept="image/png,image/jpeg,image/webp,.jpg,.jpeg,.png"
                        onChange={e => handleCardFile(e, 'front')}
                        className="hidden"
                      />
                      {!frontProcessed ? (
                        <div className="text-center p-2">
                          <div className="text-2xl">📸</div>
                          <div className="text-xs font-semibold text-slate-200 mt-1">{t.frontTitle}</div>
                          <div className="text-[10px] text-slate-500">{t.frontHint}</div>
                        </div>
                      ) : (
                        <>
                          <img
                            src={frontProcessed.src}
                            alt="Front"
                            className="w-full h-full object-cover"
                          />
                          <div className="absolute bottom-1.5 right-1.5 flex gap-1 z-10">
                            <button
                              type="button"
                              onClick={e => openCropModal('front', e)}
                              className="bg-slate-900/90 hover:bg-slate-800 text-white border border-slate-700 px-1.5 py-1 rounded text-[10px] font-semibold"
                            >
                              {t.cropBtn}
                            </button>
                            <button
                              type="button"
                              onClick={e => {
                                e.stopPropagation()
                                setFrontRotation(r => (r + 90) % 360)
                              }}
                              className="bg-slate-900/90 hover:bg-slate-800 text-white border border-slate-700 px-1.5 py-1 rounded text-[10px] font-semibold"
                            >
                              ↻ 90°
                            </button>
                            <button
                              type="button"
                              onClick={e => {
                                e.stopPropagation()
                                if (frontInputRef.current) frontInputRef.current.click()
                              }}
                              className="bg-blue-600 hover:bg-blue-500 text-white px-1.5 py-1 rounded text-[10px] font-semibold"
                            >
                              {t.btnChange}
                            </button>
                          </div>
                        </>
                      )}
                    </div>
                  )}

                  {/* Back Card */}
                  {(cardLayout === 'both' || cardLayout === 'back_only') && (
                    <div
                      onClick={() => {
                        if (!backProcessed && backInputRef.current) {
                          backInputRef.current.click()
                        }
                      }}
                      className="border-2 border-dashed border-[#1e293b] hover:border-blue-500 rounded-xl aspect-[1.586] flex flex-col items-center justify-center relative bg-[#090d16] overflow-hidden cursor-pointer transition-colors"
                    >
                      <input
                        ref={backInputRef}
                        type="file"
                        accept="image/png,image/jpeg,image/webp,.jpg,.jpeg,.png"
                        onChange={e => handleCardFile(e, 'back')}
                        className="hidden"
                      />
                      {!backProcessed ? (
                        <div className="text-center p-2">
                          <div className="text-2xl">📸</div>
                          <div className="text-xs font-semibold text-slate-200 mt-1">{t.backTitle}</div>
                          <div className="text-[10px] text-slate-500">{t.backHint}</div>
                        </div>
                      ) : (
                        <>
                          <img
                            src={backProcessed.src}
                            alt="Back"
                            className="w-full h-full object-cover"
                          />
                          <div className="absolute bottom-1.5 right-1.5 flex gap-1 z-10">
                            <button
                              type="button"
                              onClick={e => openCropModal('back', e)}
                              className="bg-slate-900/90 hover:bg-slate-800 text-white border border-slate-700 px-1.5 py-1 rounded text-[10px] font-semibold"
                            >
                              {t.cropBtn}
                            </button>
                            <button
                              type="button"
                              onClick={e => {
                                e.stopPropagation()
                                setBackRotation(r => (r + 90) % 360)
                              }}
                              className="bg-slate-900/90 hover:bg-slate-800 text-white border border-slate-700 px-1.5 py-1 rounded text-[10px] font-semibold"
                            >
                              ↻ 90°
                            </button>
                            <button
                              type="button"
                              onClick={e => {
                                e.stopPropagation()
                                if (backInputRef.current) backInputRef.current.click()
                              }}
                              className="bg-blue-600 hover:bg-blue-500 text-white px-1.5 py-1 rounded text-[10px] font-semibold"
                            >
                              {t.btnChange}
                            </button>
                          </div>
                        </>
                      )}
                    </div>
                  )}
                </div>

                {/* Xerox Page Placement */}
                <div className="mb-4">
                  <label className="block text-xs font-semibold text-slate-200 mb-1.5">
                    {t.placementLabel}
                  </label>
                  <div className="grid grid-cols-2 gap-2">
                    <button
                      type="button"
                      onClick={() => setPlacement('top')}
                      className={`py-2 px-2 rounded-lg text-xs font-semibold border transition-all text-center ${
                        placement === 'top'
                          ? 'border-blue-500 bg-blue-500/10 text-blue-400'
                          : 'border-[#1e293b] bg-[#090d16] text-slate-400 hover:border-slate-700'
                      }`}
                    >
                      ⬆️ {t.placeTop}
                    </button>
                    <button
                      type="button"
                      onClick={() => setPlacement('center')}
                      className={`py-2 px-2 rounded-lg text-xs font-semibold border transition-all text-center ${
                        placement === 'center'
                          ? 'border-blue-500 bg-blue-500/10 text-blue-400'
                          : 'border-[#1e293b] bg-[#090d16] text-slate-400 hover:border-slate-700'
                      }`}
                    >
                      🎯 {t.placeCenter}
                    </button>
                  </div>
                </div>

                {/* A4 Xerox Sheet Preview Box */}
                <div className="bg-[#090d16] border border-[#1e293b] rounded-xl p-3 mb-4">
                  <div className="flex items-center justify-between text-xs font-semibold mb-2">
                    <span className="text-slate-300">{t.a4PreviewTitle}</span>
                    <span className="text-blue-400">{t.a4PreviewSub}</span>
                  </div>
                  <div className="max-w-[210px] mx-auto shadow-2xl rounded overflow-hidden">
                    <canvas
                      ref={a4CanvasRef}
                      width={595}
                      height={842}
                      className="w-full h-auto block bg-white"
                    />
                  </div>
                  <p className="text-[11px] text-slate-400 text-center mt-2.5">
                    {t.a4LayoutDesc}
                  </p>
                </div>
              </div>
            )}

            {/* Document Mode Section */}
            {mode === 'doc' && (
              <div>
                {!selectedDocFile ? (
                  <div
                    onClick={() => docInputRef.current?.click()}
                    className="border-2 border-dashed border-[#1e293b] hover:border-blue-500 rounded-xl p-7 text-center bg-[#090d16] cursor-pointer mb-4 transition-colors"
                  >
                    <input
                      ref={docInputRef}
                      type="file"
                      accept=".pdf,image/png,image/jpeg,image/webp"
                      onChange={handleDocFile}
                      className="hidden"
                    />
                    <div className="text-3xl mb-1">📁</div>
                    <div className="text-sm font-semibold text-slate-200">{t.uploadDocTitle}</div>
                    <div className="text-xs text-slate-500 mt-0.5">{t.uploadDocSub}</div>
                  </div>
                ) : (
                  <div className="flex items-center justify-between p-3 bg-[#090d16] border border-[#1e293b] rounded-xl mb-4">
                    <div className="flex items-center gap-2.5 min-w-0">
                      {docPreviewUrl ? (
                        <img
                          src={docPreviewUrl}
                          alt="Doc"
                          className="w-12 h-12 object-cover rounded-lg border border-[#1e293b]"
                        />
                      ) : (
                        <div className="w-12 h-12 rounded-lg bg-blue-600/10 border border-blue-500/20 flex items-center justify-center text-xl text-blue-400 font-bold">
                          📄
                        </div>
                      )}
                      <div className="min-w-0">
                        <div className="text-xs font-semibold text-slate-200 truncate max-w-[190px]">
                          {selectedDocFile.name}
                        </div>
                        <div className="text-[11px] text-slate-400">
                          {(selectedDocFile.size / (1024 * 1024)).toFixed(2)} MB · Ready to print
                        </div>
                      </div>
                    </div>
                    <div className="flex items-center gap-1.5">
                      {docImageObj && (
                        <button
                          type="button"
                          onClick={e => openCropModal('doc', e)}
                          className="bg-blue-600 hover:bg-blue-500 text-white px-2 py-1 rounded-lg text-xs font-semibold"
                        >
                          {t.cropBtn}
                        </button>
                      )}
                      <button
                        type="button"
                        onClick={() => {
                          setSelectedDocFile(null)
                          setDocImageObj(null)
                          setDocPreviewUrl('')
                        }}
                        className="bg-red-500/15 hover:bg-red-500/25 text-red-400 border border-red-500/30 px-2 py-1 rounded-lg text-xs font-bold"
                      >
                        ✕
                      </button>
                    </div>
                  </div>
                )}

                {/* Print Sides (Duplex) */}
                <div className="mb-4">
                  <label className="block text-xs font-semibold text-slate-200 mb-1.5">
                    {t.docSidesLabel}
                  </label>
                  <div className="grid grid-cols-2 gap-2">
                    <button
                      type="button"
                      onClick={() => setDocSides('single')}
                      className={`py-2 px-2 rounded-lg text-xs font-semibold border transition-all text-center ${
                        docSides === 'single'
                          ? 'border-blue-500 bg-blue-500/10 text-blue-400'
                          : 'border-[#1e293b] bg-[#090d16] text-slate-400 hover:border-slate-700'
                      }`}
                    >
                      📄 {t.sideSingle}
                    </button>
                    <button
                      type="button"
                      onClick={() => setDocSides('double')}
                      className={`py-2 px-2 rounded-lg text-xs font-semibold border transition-all text-center ${
                        docSides === 'double'
                          ? 'border-blue-500 bg-blue-500/10 text-blue-400'
                          : 'border-[#1e293b] bg-[#090d16] text-slate-400 hover:border-slate-700'
                      }`}
                    >
                      📑 {t.sideDouble}
                    </button>
                  </div>
                </div>
              </div>
            )}

            {/* Adobe Scan Enhancement Filter */}
            <div className="bg-[#090d16] border border-[#1e293b] rounded-xl p-3 mb-4">
              <div className="flex items-center justify-between text-xs font-semibold mb-2">
                <span className="text-slate-300">{t.filterLabel}</span>
                <span className="text-blue-400">{t.filterSub}</span>
              </div>
              <div className="grid grid-cols-3 gap-1.5">
                <button
                  type="button"
                  onClick={() => setDocFilter('bw')}
                  className={`py-2 px-1 rounded-lg border text-center transition-all ${
                    docFilter === 'bw'
                      ? 'border-blue-500 bg-blue-500/10 text-blue-400 font-semibold'
                      : 'border-[#1e293b] bg-[#131b2e] text-slate-400'
                  }`}
                >
                  <div className="text-xs">✨ {t.filterBwTitle}</div>
                  <div className="text-[10px] text-slate-500 font-normal">{t.filterBwSub}</div>
                </button>
                <button
                  type="button"
                  onClick={() => setDocFilter('color')}
                  className={`py-2 px-1 rounded-lg border text-center transition-all ${
                    docFilter === 'color'
                      ? 'border-blue-500 bg-blue-500/10 text-blue-400 font-semibold'
                      : 'border-[#1e293b] bg-[#131b2e] text-slate-400'
                  }`}
                >
                  <div className="text-xs">🎨 {t.filterColorTitle}</div>
                  <div className="text-[10px] text-slate-500 font-normal">{t.filterColorSub}</div>
                </button>
                <button
                  type="button"
                  onClick={() => setDocFilter('original')}
                  className={`py-2 px-1 rounded-lg border text-center transition-all ${
                    docFilter === 'original'
                      ? 'border-blue-500 bg-blue-500/10 text-blue-400 font-semibold'
                      : 'border-[#1e293b] bg-[#131b2e] text-slate-400'
                  }`}
                >
                  <div className="text-xs">📷 {t.filterOrigTitle}</div>
                  <div className="text-[10px] text-slate-500 font-normal">{t.filterOrigSub}</div>
                </button>
              </div>
            </div>

            {/* Paper Size & Orientation */}
            <div className="grid grid-cols-2 gap-3 mb-4">
              <div>
                <label className="block text-xs font-semibold text-slate-200 mb-1.5">
                  {t.paperSizeLabel}
                </label>
                <select
                  value={paperSize}
                  onChange={e => setPaperSize(e.target.value as 'A4' | 'Legal' | '4x6')}
                  className="w-full h-10 px-3 bg-[#090d16] border border-[#1e293b] rounded-xl text-xs font-semibold text-white outline-none focus:border-blue-500"
                >
                  <option value="A4">A4 (Standard Xerox)</option>
                  <option value="Legal">Legal (8.5×14")</option>
                  <option value="4x6">4×6 Photo</option>
                </select>
              </div>

              <div>
                <label className="block text-xs font-semibold text-slate-200 mb-1.5">
                  {t.orientationLabel}
                </label>
                <div className="grid grid-cols-2 h-10 border border-[#1e293b] rounded-xl overflow-hidden bg-[#090d16]">
                  <button
                    type="button"
                    onClick={() => setOrientation('portrait')}
                    className={`text-xs font-semibold transition-colors ${
                      orientation === 'portrait' ? 'bg-blue-600 text-white' : 'text-slate-400 hover:text-white'
                    }`}
                  >
                    {t.portrait}
                  </button>
                  <button
                    type="button"
                    onClick={() => setOrientation('landscape')}
                    className={`text-xs font-semibold transition-colors ${
                      orientation === 'landscape' ? 'bg-blue-600 text-white' : 'text-slate-400 hover:text-white'
                    }`}
                  >
                    {t.landscape}
                  </button>
                </div>
              </div>
            </div>

            {/* Copies & Color Mode */}
            <div className="grid grid-cols-2 gap-3 mb-5">
              <div>
                <label className="block text-xs font-semibold text-slate-200 mb-1.5">
                  {t.copiesLabel}
                </label>
                <div className="flex items-center h-10 border border-[#1e293b] rounded-xl overflow-hidden bg-[#090d16]">
                  <button
                    type="button"
                    onClick={() => setCopies(c => Math.max(1, c - 1))}
                    className="w-10 h-full text-base font-bold text-slate-300 hover:bg-slate-800"
                  >
                    -
                  </button>
                  <div className="flex-1 text-center font-bold text-sm text-white">{copies}</div>
                  <button
                    type="button"
                    onClick={() => setCopies(c => Math.min(99, c + 1))}
                    className="w-10 h-full text-base font-bold text-slate-300 hover:bg-slate-800"
                  >
                    +
                  </button>
                </div>
              </div>

              <div>
                <label className="block text-xs font-semibold text-slate-200 mb-1.5">
                  {t.colorModeLabel}
                </label>
                <div className="grid grid-cols-2 h-10 border border-[#1e293b] rounded-xl overflow-hidden bg-[#090d16]">
                  <button
                    type="button"
                    onClick={() => setColorMode('bw')}
                    className={`text-xs font-semibold transition-colors ${
                      colorMode === 'bw' ? 'bg-blue-600 text-white' : 'text-slate-400 hover:text-white'
                    }`}
                  >
                    {t.btnBw}
                  </button>
                  <button
                    type="button"
                    onClick={() => setColorMode('color')}
                    className={`text-xs font-semibold transition-colors ${
                      colorMode === 'color' ? 'bg-blue-600 text-white' : 'text-slate-400 hover:text-white'
                    }`}
                  >
                    {t.btnColor}
                  </button>
                </div>
              </div>
            </div>

            {/* Send to Printer Button */}
            <button
              type="button"
              disabled={isSubmitting}
              onClick={handleSubmit}
              className="w-full py-3.5 bg-blue-600 hover:bg-blue-500 disabled:opacity-50 text-white font-bold text-sm rounded-xl flex items-center justify-center gap-2 shadow-lg shadow-blue-600/30 transition-all cursor-pointer"
            >
              {isSubmitting ? (
                <>
                  <span className="w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />
                  <span>{submitProgress > 50 ? t.uploadingFile : t.submitting} ({submitProgress}%)</span>
                </>
              ) : (
                <span>{t.submitBtn}</span>
              )}
            </button>

            {/* Footer Notice */}
            <p className="text-[11px] text-slate-400 text-center mt-3.5 leading-relaxed">
              {t.privacyFooter}
            </p>
          </div>
        ) : (
          /* Success Screen */
          <div className="bg-[#131b2e] border border-[#1e293b] rounded-2xl p-6 text-center shadow-2xl animate-in fade-in zoom-in-95 duration-300">
            <div className="w-14 h-14 bg-emerald-500/15 border border-emerald-500/30 text-emerald-400 rounded-full flex items-center justify-center text-2xl mx-auto mb-3 shadow-lg shadow-emerald-500/10">
              ✓
            </div>
            <h2 className="text-lg font-bold text-white mb-1">{t.successTitle}</h2>
            <p className="text-xs text-slate-400 mb-4">{t.successSub}</p>

            <div className="bg-[#090d16] border border-[#1e293b] rounded-xl p-3.5 text-left text-xs mb-4 space-y-2">
              <div className="flex justify-between">
                <span className="text-slate-400">{t.resCustomerLabel}</span>
                <span className="font-semibold text-white">{successJob.customer}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-slate-400">{t.resShopLabel}</span>
                <span className="font-semibold text-blue-400">{successJob.shop}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-slate-400">{t.resDetailsLabel}</span>
                <span className="font-semibold text-white">{successJob.details}</span>
              </div>
              <div className="flex justify-between pt-1 border-t border-slate-800">
                <span className="text-slate-400">{t.resStatusLabel}</span>
                <span className="font-semibold text-emerald-400 flex items-center gap-1.5">
                  <span className="w-2 h-2 rounded-full bg-emerald-400 animate-pulse" />
                  {t.resStatusVal}
                </span>
              </div>
            </div>

            {/* Google AdSense / Sponsor Placeholder */}
            <div className="bg-[#090d16] border border-dashed border-[#1e293b] rounded-xl p-3 mb-5 text-center">
              <div className="text-[9px] uppercase tracking-wider text-slate-500 font-semibold mb-0.5">
                Advertisement
              </div>
              <div className="text-xs font-semibold text-slate-200">NearbyPrint Smart Self-Service</div>
              <div className="text-[11px] text-slate-400">Fast · 100% Private · Zero WhatsApp Hassle</div>
            </div>

            <button
              type="button"
              onClick={resetForm}
              className="w-full py-2.5 bg-slate-800 hover:bg-slate-700 text-slate-200 border border-slate-700 font-semibold text-xs rounded-xl transition-colors"
            >
              {t.printAnotherBtn}
            </button>
          </div>
        )}
      </div>

      {/* Adobe Scan 4-Corner Crop Modal */}
      {cropModalOpen && (
        <div className="fixed inset-0 bg-black/95 z-50 flex flex-col touch-none">
          <div className="p-3.5 px-4 flex items-center justify-between text-white bg-slate-900 border-b border-slate-800">
            <span className="font-semibold text-sm">{t.cropHeader}</span>
            <button
              type="button"
              onClick={() => {
                setCropModalOpen(false)
                hideLoupe()
              }}
              className="text-slate-400 hover:text-white text-lg p-1"
            >
              ✕
            </button>
          </div>

          <div className="flex-1 relative flex items-center justify-center p-2.5 overflow-hidden">
            <canvas
              ref={cropCanvasRef}
              onPointerDown={handlePointerDown}
              onPointerMove={handlePointerMove}
              onPointerUp={handlePointerUp}
              className="max-w-full max-h-full block shadow-2xl cursor-crosshair touch-none"
            />
          </div>

          <div className="p-3 px-4 flex items-center justify-between bg-slate-900 border-t border-slate-800 gap-2">
            <button
              type="button"
              onClick={() => {
                if (cropImgSource) runAutoDetect(cropImgSource)
              }}
              className="bg-slate-800 hover:bg-slate-700 text-white px-3 py-2 rounded-lg text-xs font-semibold border border-slate-700"
            >
              {t.cropAutoBtn}
            </button>
            <button
              type="button"
              onClick={() => {
                if (!cropImgSource) return
                const w = 'width' in cropImgSource ? cropImgSource.width : (cropImgSource as HTMLImageElement).naturalWidth
                const h = 'height' in cropImgSource ? cropImgSource.height : (cropImgSource as HTMLImageElement).naturalHeight
                const mX = Math.round(w * 0.05)
                const mY = Math.round(h * 0.05)
                setCropCorners([
                  { x: mX, y: mY },
                  { x: w - mX, y: mY },
                  { x: w - mX, y: h - mY },
                  { x: mX, y: h - mY }
                ])
              }}
              className="bg-slate-800 hover:bg-slate-700 text-white px-3 py-2 rounded-lg text-xs font-semibold border border-slate-700"
            >
              {t.cropResetBtn}
            </button>
            <button
              type="button"
              onClick={applyCropAndClose}
              className="bg-blue-600 hover:bg-blue-500 text-white px-4 py-2 rounded-lg text-xs font-bold shadow-md shadow-blue-600/30"
            >
              {t.cropApplyBtn}
            </button>
          </div>
        </div>
      )}

      {/* Magnifying Loupe Overlay */}
      <div
        ref={cropLoupeRef}
        style={{ display: 'none' }}
        className="fixed w-[110px] h-[110px] rounded-full border-[3px] border-white shadow-2xl pointer-events-none z-[150] overflow-hidden bg-black"
      >
        <canvas
          ref={loupeCanvasRef}
          width={120}
          height={120}
          className="w-full h-full block"
        />
        <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
          <div className="w-[18px] h-[2px] bg-[#00f2fe] shadow-[0_0_4px_rgba(0,0,0,0.8)] absolute" />
          <div className="h-[18px] w-[2px] bg-[#00f2fe] shadow-[0_0_4px_rgba(0,0,0,0.8)] absolute" />
        </div>
      </div>
    </div>
  )
}
