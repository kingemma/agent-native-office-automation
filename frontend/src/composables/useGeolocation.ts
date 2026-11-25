/**
 * 定位获取：Tauri 外壳走原生插件（移动端弹系统授权），浏览器走 navigator.geolocation。
 *
 * 浏览器的 geolocation 只在安全上下文（HTTPS / localhost）可用；局域网 HTTP 会被直接拒绝，
 * 这里显式区分「不支持 / 未授权 / 非安全上下文 / 超时」，让调用方给出可操作的提示。
 */
import { isTauri } from '@tauri-apps/api/core'

export interface GeoPoint {
  latitude: number
  longitude: number
  accuracy?: number
}

export type GeoErrorKind = 'unsupported' | 'denied' | 'unavailable' | 'timeout' | 'insecure'

export class GeoError extends Error {
  constructor(
    public readonly kind: GeoErrorKind,
    message: string,
  ) {
    super(message)
    this.name = 'GeoError'
  }
}

export async function getCurrentLocation(): Promise<GeoPoint> {
  return isTauri() ? fromTauri() : fromBrowser()
}

async function fromTauri(): Promise<GeoPoint> {
  const { checkPermissions, requestPermissions, getCurrentPosition } = await import(
    '@tauri-apps/plugin-geolocation'
  )
  let permission = await checkPermissions()
  if (permission.location !== 'granted') {
    permission = await requestPermissions(['location'])
  }
  if (permission.location !== 'granted') {
    throw new GeoError('denied', '定位权限未授予')
  }
  const pos = await getCurrentPosition()
  return {
    latitude: pos.coords.latitude,
    longitude: pos.coords.longitude,
    accuracy: pos.coords.accuracy,
  }
}

function fromBrowser(): Promise<GeoPoint> {
  if (!('geolocation' in navigator)) {
    return Promise.reject(new GeoError('unsupported', '当前设备不支持定位'))
  }
  if (!window.isSecureContext) {
    return Promise.reject(new GeoError('insecure', '浏览器仅在 HTTPS 或 localhost 下允许定位'))
  }
  return new Promise((resolve, reject) => {
    navigator.geolocation.getCurrentPosition(
      (pos) => resolve({
        latitude: pos.coords.latitude,
        longitude: pos.coords.longitude,
        accuracy: pos.coords.accuracy,
      }),
      (err) => {
        const kind: GeoErrorKind =
          err.code === err.PERMISSION_DENIED
            ? 'denied'
            : err.code === err.TIMEOUT
              ? 'timeout'
              : 'unavailable'
        reject(new GeoError(kind, err.message))
      },
      { enableHighAccuracy: true, timeout: 10000, maximumAge: 0 },
    )
  })
}
