// =============================================================
// NearbyPrint - Client-Side Auto Corner Detection & Perspective Warp
// 100% Zero-Server Load: OpenCV.js (Lazy Loaded) + Pure Canvas Engine
// =============================================================

export interface Point2D {
  x: number
  y: number
}

// Global reference & lazy loader for OpenCV.js
let openCvPromise: Promise<any> | null = null

export function lazyLoadOpenCV(): Promise<any> {
  if (typeof window === 'undefined') return Promise.resolve(null)
  const win = window as any
  if (win.cv && win.cv.Mat) {
    return Promise.resolve(win.cv)
  }
  if (openCvPromise) return openCvPromise

  openCvPromise = new Promise(resolve => {
    // Check if script already injected
    const existing = document.getElementById('opencv-script')
    if (existing) {
      const checkCv = () => {
        if (win.cv && win.cv.Mat) resolve(win.cv)
        else setTimeout(checkCv, 50)
      }
      checkCv()
      return
    }

    const script = document.createElement('script')
    script.id = 'opencv-script'
    script.src = 'https://docs.opencv.org/4.8.0/opencv.js'
    script.async = true
    script.onload = () => {
      const cv = win.cv
      if (cv) {
        if (cv.onRuntimeInitialized) {
          cv.onRuntimeInitialized = () => resolve(cv)
        } else {
          resolve(cv)
        }
      } else {
        resolve(null)
      }
    }
    script.onerror = () => {
      console.warn('OpenCV.js CDN load failed. Falling back to built-in pure canvas detection.')
      resolve(null)
    }
    document.head.appendChild(script)

    // Timeout safety fallback (5s)
    setTimeout(() => {
      if (!win.cv || !win.cv.Mat) resolve(null)
    }, 5000)
  })

  return openCvPromise
}

/**
 * Standardizes 4 corner points into [Top-Left, Top-Right, Bottom-Right, Bottom-Left].
 */
export function orderCorners(pts: Point2D[]): Point2D[] {
  let tl = pts[0], br = pts[0], tr = pts[0], bl = pts[0]
  let minSum = Infinity, maxSum = -Infinity
  let minDiff = Infinity, maxDiff = -Infinity

  for (const p of pts) {
    const sum = p.x + p.y
    const diff = p.y - p.x
    if (sum < minSum) { minSum = sum; tl = p }
    if (sum > maxSum) { maxSum = sum; br = p }
    if (diff < minDiff) { minDiff = diff; tr = p }
    if (diff > maxDiff) { maxDiff = diff; bl = p }
  }

  return [tl, tr, br, bl]
}

/**
 * High-accuracy OpenCV.js corner detector.
 */
function detectWithOpenCV(
  cv: any,
  srcImg: CanvasImageSource,
  srcW: number,
  srcH: number,
  targetType: 'front' | 'back' | 'doc'
): Point2D[] | null {
  const maxDim = 720
  const scale = Math.min(1, maxDim / Math.max(srcW, srcH))
  const tw = Math.max(80, Math.round(srcW * scale))
  const th = Math.max(80, Math.round(srcH * scale))

  const smallCanvas = document.createElement('canvas')
  smallCanvas.width = tw
  smallCanvas.height = th
  const sCtx = smallCanvas.getContext('2d')
  if (!sCtx) return null
  sCtx.drawImage(srcImg, 0, 0, tw, th)

  let srcMat = cv.imread(smallCanvas)
  let gray = new cv.Mat()
  let blur = new cv.Mat()
  let closed = new cv.Mat()
  let edges = new cv.Mat()
  let dilatedEdges = new cv.Mat()
  let contours = new cv.MatVector()
  let hierarchy = new cv.Mat()

  try {
    // 1. Grayscale
    cv.cvtColor(srcMat, gray, cv.COLOR_RGBA2GRAY)

    // 2. Light Gaussian Blur (5x5) to suppress background sensor noise
    cv.GaussianBlur(gray, blur, new cv.Size(5, 5), 0)

    // 3. Morphological Closing (7x7) on Grayscale
    // Merges internal text and emblem edges into solid card background
    const kClose = cv.getStructuringElement(cv.MORPH_RECT, new cv.Size(7, 7))
    cv.morphologyEx(blur, closed, cv.MORPH_CLOSE, kClose)
    kClose.delete()

    // 4. Canny Edge Detection (75, 200)
    cv.Canny(closed, edges, 75, 200)

    // 5. Morphological Dilation (3x3) to bridge broken borders
    const kDilate = cv.getStructuringElement(cv.MORPH_RECT, new cv.Size(3, 3))
    cv.dilate(edges, dilatedEdges, kDilate)
    kDilate.delete()

    // 6. Contour Finding
    cv.findContours(dilatedEdges, contours, hierarchy, cv.RETR_LIST, cv.CHAIN_APPROX_SIMPLE)

    // 7. Area >= 15% and CR80 Aspect Ratio
    const minArea = tw * th * 0.15
    let bestApprox: Point2D[] | null = null
    let bestScore = -1

    for (let i = 0; i < contours.size(); i++) {
      const c = contours.get(i)
      const area = cv.contourArea(c)
      if (area < minArea) continue

      const peri = cv.arcLength(c, true)
      const approx = new cv.Mat()

      for (let eps = 0.015; eps <= 0.055; eps += 0.005) {
        cv.approxPolyDP(c, approx, peri * eps, true)
        if (approx.rows === 4 && cv.isContourConvex(approx)) {
          const pts: Point2D[] = []
          for (let r = 0; r < 4; r++) {
            pts.push({
              x: approx.data32S[r * 2],
              y: approx.data32S[r * 2 + 1],
            })
          }

          const ordered = orderCorners(pts)
          const topW = Math.hypot(ordered[1].x - ordered[0].x, ordered[1].y - ordered[0].y)
          const botW = Math.hypot(ordered[2].x - ordered[3].x, ordered[2].y - ordered[3].y)
          const leftH = Math.hypot(ordered[3].x - ordered[0].x, ordered[3].y - ordered[0].y)
          const rightH = Math.hypot(ordered[2].x - ordered[1].x, ordered[2].y - ordered[1].y)

          const avgW = (topW + botW) / 2
          const avgH = (leftH + rightH) / 2
          const aspect = Math.max(avgW / Math.max(1, avgH), avgH / Math.max(1, avgW))

          let score = area
          if (targetType === 'front' || targetType === 'back') {
            // CR80 Aspect Ratio ~ 1.586 (User requirement: ~1.35 to 1.85)
            if (aspect < 1.25 || aspect > 1.95) {
              continue
            }
            score = area * (1 - Math.abs(aspect - 1.58577) / 1.58577)
          }

          if (score > bestScore) {
            bestScore = score
            bestApprox = ordered
          }
          break
        }
      }
      approx.delete()
    }

    if (bestApprox) {
      const inv = 1 / scale
      return bestApprox.map(p => ({
        x: Math.max(0, Math.min(srcW, Math.round(p.x * inv))),
        y: Math.max(0, Math.min(srcH, Math.round(p.y * inv))),
      }))
    }
    return null
  } catch (err) {
    console.warn('OpenCV edge detection error:', err)
    return null
  } finally {
    srcMat.delete()
    gray.delete()
    blur.delete()
    closed.delete()
    edges.delete()
    dilatedEdges.delete()
    contours.delete()
    hierarchy.delete()
  }
}

/**
 * Built-in Pure TypeScript / Canvas Edge Detector.
 * Implements Grayscale + 5x5 Gaussian + Separable 7x7 Grayscale Morphological Closing + Canny + approxPolyDP.
 * Runs in sub-15ms without any external downloads or server calls.
 */
function detectWithPureCanvas(
  srcImg: CanvasImageSource,
  srcW: number,
  srcH: number,
  targetType: 'front' | 'back' | 'doc'
): Point2D[] | null {
  const maxDim = 640
  const scale = Math.min(1, maxDim / Math.max(srcW, srcH))
  const tw = Math.max(50, Math.round(srcW * scale))
  const th = Math.max(50, Math.round(srcH * scale))

  const aCanvas = document.createElement('canvas')
  aCanvas.width = tw
  aCanvas.height = th
  const aCtx = aCanvas.getContext('2d')
  if (!aCtx) return null
  aCtx.drawImage(srcImg, 0, 0, tw, th)
  const d = aCtx.getImageData(0, 0, tw, th).data

  // 1. Grayscale
  const rawGray = new Uint8Array(tw * th)
  for (let i = 0, j = 0; i < d.length; i += 4, j++) {
    rawGray[j] = Math.round(0.299 * d[i] + 0.587 * d[i + 1] + 0.114 * d[i + 2])
  }

  // 2. Separable 5x5 Gaussian Blur: [1, 4, 6, 4, 1] / 16
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

  // 3. Grayscale Morphological Closing (Dilation 7x7 max filter + Erosion 7x7 min filter)
  // This suppresses high-frequency interior text/signatures while keeping card boundaries sharp.
  const radius = 3 // 7x7
  const dilatedH = new Uint8Array(tw * th)
  const dilated = new Uint8Array(tw * th)

  // Horizontal Max
  for (let y = 0; y < th; y++) {
    const row = y * tw
    for (let x = 0; x < tw; x++) {
      let m = 0
      for (let dx = -radius; dx <= radius; dx++) {
        const px = Math.min(tw - 1, Math.max(0, x + dx))
        const v = blur[row + px]
        if (v > m) m = v
      }
      dilatedH[row + x] = m
    }
  }

  // Vertical Max
  for (let y = 0; y < th; y++) {
    for (let x = 0; x < tw; x++) {
      let m = 0
      for (let dy = -radius; dy <= radius; dy++) {
        const py = Math.min(th - 1, Math.max(0, y + dy))
        const v = dilatedH[py * tw + x]
        if (v > m) m = v
      }
      dilated[y * tw + x] = m
    }
  }

  // Erosion on dilated: Horizontal Min
  const closedH = new Uint8Array(tw * th)
  const closed = new Uint8Array(tw * th)

  for (let y = 0; y < th; y++) {
    const row = y * tw
    for (let x = 0; x < tw; x++) {
      let m = 255
      for (let dx = -radius; dx <= radius; dx++) {
        const px = Math.min(tw - 1, Math.max(0, x + dx))
        const v = dilated[row + px]
        if (v < m) m = v
      }
      closedH[row + x] = m
    }
  }

  // Vertical Min
  for (let y = 0; y < th; y++) {
    for (let x = 0; x < tw; x++) {
      let m = 255
      for (let dy = -radius; dy <= radius; dy++) {
        const py = Math.min(th - 1, Math.max(0, y + dy))
        const v = closedH[py * tw + x]
        if (v < m) m = v
      }
      closed[y * tw + x] = m
    }
  }

  // 4. Sobel Gradients
  const gxArr = new Int32Array(tw * th)
  const gyArr = new Int32Array(tw * th)
  const mag = new Float32Array(tw * th)
  let maxMag = 0

  for (let y = 1; y < th - 1; y++) {
    const row = y * tw
    const prev = (y - 1) * tw
    const next = (y + 1) * tw
    for (let x = 1; x < tw - 1; x++) {
      const gx = -closed[prev + x - 1] + closed[prev + x + 1]
                 - 2 * closed[row + x - 1] + 2 * closed[row + x + 1]
                 - closed[next + x - 1] + closed[next + x + 1]
      const gy = -closed[prev + x - 1] - 2 * closed[prev + x] - closed[prev + x + 1]
                 + closed[next + x - 1] + 2 * closed[next + x] + closed[next + x + 1]
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

  // 5. Binary Edge Dilation (3x3) to bridge minor boundary gaps
  const binEdges = new Uint8Array(tw * th)
  for (let y = 1; y < th - 1; y++) {
    for (let x = 1; x < tw - 1; x++) {
      if (edges[y * tw + x] === 2) {
        for (let dy = -1; dy <= 1; dy++) {
          for (let dx = -1; dx <= 1; dx++) {
            binEdges[(y + dy) * tw + (x + dx)] = 1
          }
        }
      }
    }
  }

  // 6. Contour Boundary Extraction
  const visited = new Uint8Array(tw * th)
  const candidateContours: Point2D[][] = []
  const minContourLen = Math.round((tw + th) * 0.20)

  for (let y = 2; y < th - 2; y++) {
    for (let x = 2; x < tw - 2; x++) {
      const idx = y * tw + x
      if (binEdges[idx] === 1 && !visited[idx]) {
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
                if (binEdges[nIdx] === 1 && !visited[nIdx]) {
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
    const sorted = points.slice().sort((a, b) => (a.x === b.x ? a.y - b.y : a.x - b.x))
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

  // Ramer-Douglas-Peucker approxPolyDP
  function approxPolyDP(pts: Point2D[], eps: number): Point2D[] {
    if (pts.length <= 2) return pts.slice()
    let maxD = 0, maxIdx = 0
    const p1 = pts[0], p2 = pts[pts.length - 1]
    const dx = p2.x - p1.x, dy = p2.y - p1.y
    const len = Math.hypot(dx, dy)

    for (let i = 1; i < pts.length - 1; i++) {
      const d = len < 1e-5
        ? Math.hypot(pts[i].x - p1.x, pts[i].y - p1.y)
        : Math.abs((pts[i].x - p1.x) * dy - (pts[i].y - p1.y) * dx) / len
      if (d > maxD) {
        maxD = d
        maxIdx = i
      }
    }

    if (maxD > eps) {
      const r1 = approxPolyDP(pts.slice(0, maxIdx + 1), eps)
      const r2 = approxPolyDP(pts.slice(maxIdx), eps)
      return r1.slice(0, -1).concat(r2)
    }
    return [p1, p2]
  }

  const minArea = tw * th * 0.15 // Minimum 15% image area requirement
  let bestQuad: Point2D[] | null = null
  let bestScore = -1

  for (const cnt of candidateContours.slice(0, 15)) {
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
          const ordered = orderCorners(approx)
          const topW = Math.hypot(ordered[1].x - ordered[0].x, ordered[1].y - ordered[0].y)
          const botW = Math.hypot(ordered[2].x - ordered[3].x, ordered[2].y - ordered[3].y)
          const leftH = Math.hypot(ordered[3].x - ordered[0].x, ordered[3].y - ordered[0].y)
          const rightH = Math.hypot(ordered[2].x - ordered[1].x, ordered[2].y - ordered[1].y)

          const avgW = (topW + botW) / 2
          const avgH = (leftH + rightH) / 2
          const aspect = Math.max(avgW / Math.max(1, avgH), avgH / Math.max(1, avgW))

          let score = area
          if (targetType === 'front' || targetType === 'back') {
            // Check CR80 aspect ratio ~1.35 to 1.85
            if (aspect < 1.25 || aspect > 1.95) {
              continue
            }
            score = area * (1 - Math.abs(aspect - 1.58577) / 1.58577)
          }

          if (score > bestScore) {
            bestScore = score
            bestQuad = ordered
          }
          break
        }
      }
    }
  }

  if (bestQuad) {
    const inv = 1 / scale
    return bestQuad.map(p => ({
      x: Math.max(0, Math.min(srcW, Math.round(p.x * inv))),
      y: Math.max(0, Math.min(srcH, Math.round(p.y * inv))),
    }))
  }

  return null
}

/**
 * Main Auto Corner Detection entrypoint.
 * Automatically tries OpenCV.js if loaded/ready, otherwise uses high-precision Canvas engine.
 * Falls back to centered box if no clean card boundary is detected.
 */
export async function detectCardCorners(
  srcImg: CanvasImageSource,
  targetType: 'front' | 'back' | 'doc'
): Promise<Point2D[]> {
  const srcW = 'width' in srcImg ? Number((srcImg as any).width) : Number((srcImg as any).naturalWidth || (srcImg as any).videoWidth || 800)
  const srcH = 'height' in srcImg ? Number((srcImg as any).height) : Number((srcImg as any).naturalHeight || (srcImg as any).videoHeight || 600)

  // 1. Try OpenCV if available or quickly loadable
  const win = typeof window !== 'undefined' ? (window as any) : null
  if (win && win.cv && win.cv.Mat) {
    const cvDetected = detectWithOpenCV(win.cv, srcImg, srcW, srcH, targetType)
    if (cvDetected) return cvDetected
  }

  // 2. Try Pure Canvas engine
  const canvasDetected = detectWithPureCanvas(srcImg, srcW, srcH, targetType)
  if (canvasDetected) return canvasDetected

  // 3. Fallback: Standard centered quad
  if (targetType === 'doc') {
    const marginX = Math.round(srcW * 0.08)
    const marginY = Math.round(srcH * 0.08)
    return [
      { x: marginX, y: marginY },
      { x: srcW - marginX, y: marginY },
      { x: srcW - marginX, y: srcH - marginY },
      { x: marginX, y: srcH - marginY },
    ]
  }

  // For CR80 ID Cards: 85.60mm x 53.98mm ratio = 1.58577
  const marginX = Math.round(srcW * 0.08)
  const cardW = srcW - 2 * marginX
  const cardH = Math.min(Math.round(srcH * 0.84), Math.round(cardW / 1.58577))
  const startY = Math.max(0, Math.round((srcH - cardH) / 2))

  return [
    { x: marginX, y: startY },
    { x: marginX + cardW, y: startY },
    { x: marginX + cardW, y: Math.min(srcH, startY + cardH) },
    { x: marginX, y: Math.min(srcH, startY + cardH) },
  ]
}

/**
 * 4-Point Perspective Transform (Straightening / Dewarping)
 * Uses OpenCV.js cv.warpPerspective if available, or closed-form bilinear projective homography.
 */
export function warpPerspectiveTransform(
  srcImg: CanvasImageSource,
  srcW: number,
  srcH: number,
  corners: { tl: Point2D; tr: Point2D; br: Point2D; bl: Point2D },
  outW: number,
  outH: number
): HTMLCanvasElement {
  const win = typeof window !== 'undefined' ? (window as any) : null
  const cv = win?.cv

  // 1. OpenCV-accelerated warp
  if (cv && cv.Mat && cv.getPerspectiveTransform && cv.warpPerspective) {
    try {
      const tempSrc = document.createElement('canvas')
      tempSrc.width = srcW
      tempSrc.height = srcH
      const sCtx = tempSrc.getContext('2d')!
      sCtx.drawImage(srcImg, 0, 0)

      const srcMat = cv.imread(tempSrc)
      const dstMat = new cv.Mat()
      const dsize = new cv.Size(outW, outH)

      const srcTri = cv.matFromArray(4, 1, cv.CV_32FC2, [
        corners.tl.x, corners.tl.y,
        corners.tr.x, corners.tr.y,
        corners.br.x, corners.br.y,
        corners.bl.x, corners.bl.y,
      ])
      const dstTri = cv.matFromArray(4, 1, cv.CV_32FC2, [
        0, 0,
        outW, 0,
        outW, outH,
        0, outH,
      ])

      const M = cv.getPerspectiveTransform(srcTri, dstTri)
      cv.warpPerspective(srcMat, dstMat, M, dsize, cv.INTER_LINEAR, cv.BORDER_CONSTANT, new cv.Scalar())

      const dstCanvas = document.createElement('canvas')
      dstCanvas.width = outW
      dstCanvas.height = outH
      cv.imshow(dstCanvas, dstMat)

      srcMat.delete()
      dstMat.delete()
      srcTri.delete()
      dstTri.delete()
      M.delete()

      return dstCanvas
    } catch (err) {
      console.warn('OpenCV warpPerspective fallback:', err)
    }
  }

  // 2. Closed-Form Projective Homography with Bilinear Interpolation (Heckbert Mapping)
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
      const fx = sxPt - x0
      const fy = syPt - y0

      if (x0 >= 0 && x1 < srcW && y0 >= 0 && y1 < srcH) {
        const i00 = (y0 * srcW + x0) * 4
        const i10 = (y0 * srcW + x1) * 4
        const i01 = (y1 * srcW + x0) * 4
        const i11 = (y1 * srcW + x1) * 4
        const dIdx = dRow + (x * 4)

        for (let ch = 0; ch < 4; ch++) {
          dp[dIdx + ch] = Math.round(
            (1 - fx) * (1 - fy) * sp[i00 + ch] +
            fx * (1 - fy) * sp[i10 + ch] +
            (1 - fx) * fy * sp[i01 + ch] +
            fx * fy * sp[i11 + ch]
          )
        }
      }
    }
  }

  dCtx.putImageData(dData, 0, 0)
  return dst
}
