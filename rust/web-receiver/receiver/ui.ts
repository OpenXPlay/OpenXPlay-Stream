// SPDX-FileCopyrightText: 2026 OpenXPlay.org
// SPDX-License-Identifier: AGPL-3.0-only
// Created by riko chen on 2026-10-14

import { frameRatePreference } from './frame-rate';
import { videoCodecPreference } from '../shared/video-codec';
import { playbackMode } from '../shared/playback';
import type { FrameRatePreference, PlaybackMode, VideoCodecPreference } from '../protocol/json';

export type Language = 'en' | 'zh-CN';
export function selectLanguage(languages: readonly string[]): Language {
  for (const language of languages) {
    if (/^zh(?:$|-(?:CN|SG)(?:-|$)|-Hans(?:-|$))/i.test(language)) return 'zh-CN';
    if (/^en(?:-|$)/i.test(language)) return 'en';
  }
  return 'en';
}
export const strings = {
  en: {
    resolution: 'Resolution', native: 'Native', wholeScreen: 'Whole screen', nativeHint: 'Native is sharpest and needs more Wi-Fi bandwidth.', screenWidth: 'CarPlay screen width', screenWidthHint: 'Larger values show more icons on one page.', fullscreenRestore: 'Tap to return to full screen', fullscreenHint: 'Tap to return to full screen',
    frameRate: 'Frame rate', automatic: 'Automatic', applying: 'Applying. CarPlay reconnects in a few seconds',
    videoCodec: 'Video codec', codecAuto: 'Automatic (recommended)', codecH264: 'Force H.264', codecH265: 'Force H.265',
    codecHint: 'Automatic uses H.265 when this browser can decode it in hardware and falls back to H.264 if it fails. The forced choices are for testing.',
    codecInUseH264: 'Now using H.264.', codecInUseH265: 'Now using H.265.',
    playback: 'Playback', smart: 'Smart', quality: 'Quality', speed: 'Speed',
    smartHint: 'Balances smoothness and delay for your network.', qualityHint: 'Smoothest motion, with a little more delay.',
    speedHint: 'Lowest delay; frames may be skipped.',
    picture: 'Picture', windowSize: 'Window', back: 'Back', soundAndMicrophone: 'Sound and microphone', plugins: 'Plugins', resetPluginPositions: 'Reset plugin positions', relayVersion: 'version',
    connectingIphone: 'Connecting iPhone', connectingIphoneHint: 'Keep your iPhone nearby while CarPlay connects.',
    authenticating: 'Preparing CarPlay', authenticatingHint: 'Your iPhone is being authenticated.',
    joiningNetwork: 'Connecting iPhone to Wi-Fi', joiningNetworkHint: 'Keep the relay hotspot or car network available.',
    waitingMedia: 'Waiting for picture', waitingMediaHint: 'Your iPhone has connected. Waiting for the CarPlay picture.',
    phoneFailed: 'iPhone connection interrupted', phoneFailedHint: 'Check your iPhone and keep the relay on while it retries.', settings: 'Settings', close: 'Close', sound: 'Sound', microphone: 'Microphone', fit: 'Picture fit',
    contain: 'Fit', fill: 'Fill', fullscreen: 'Full screen', developer: 'Developer mode', about: 'About',
    source: 'Source', version: 'Web version', powered: 'Powered by OpenXPlay.', soundHint: 'Tap for sound',
    code: 'Enter pairing code', codeHint: 'Enter the 8-digit code shown on the relay.', incorrect: 'Incorrect code',
    retryIn: 'Trying again in {seconds} s. Check your relay and network.', connect: 'Connect', connecting: 'Connecting', connectingHint: 'Connecting to your relay…',
    waiting: 'Waiting for iPhone', waitingHint: 'Connect your iPhone to the relay to start CarPlay.',
    reconnecting: 'Reconnecting', reconnectingHint: 'Keep the relay on and check your network connection.',
    error: 'Relay unreachable', errorHint: 'Keep the relay on and join the same network, then try again.', retry: 'Try again',
    unsupported: 'Video not supported', unsupportedHint: 'Try another browser and open the relay’s secure (HTTPS) address.',
    idle: 'Not requested', capturing: 'Microphone is on', requesting: 'Requesting permission…',
    denied: 'Permission denied or unavailable. Enable microphone access in browser settings.',
    unavailable: 'Microphone unavailable in this browser.', disabled: 'Microphone is off',
    projection: 'CarPlay video',
    notInUse: 'Not in use', listening: 'Listening', blocked: 'Blocked',
    micIdleHint: 'Used only when CarPlay asks for your voice.',
    micListeningHint: 'CarPlay is using your microphone.',
    micBlockedHint: 'Allow microphone access in this site’s browser settings, then try again.',
    micUnavailableHint: 'Use a browser with microphone support over HTTPS.',
    micRequestHint: 'Choose Allow in the browser permission prompt.',
    enabled: 'On', off: 'Off', fullscreenUnavailable: 'Unavailable in this browser',
    soundBlocked: 'Sound is paused', soundBlockedHint: 'Tap to allow sound in this browser.',
    microphoneDenied: 'Microphone blocked', microphoneDeniedHint: 'Allow microphone access in browser settings.',
    playing: 'CarPlay is running', playingHint: 'Use CarPlay on this screen.', paired: 'Code accepted',
    attempt: 'Attempt {attempt} · {seconds} s until retry. Check your relay and network.',
  },
  'zh-CN': {
    resolution: '分辨率', native: '原画', wholeScreen: '整个屏幕', nativeHint: '原画最清晰，需要更多 Wi-Fi 带宽。', screenWidth: 'CarPlay 屏幕宽度', screenWidthHint: '数值越大，一页显示的图标越多。', fullscreenRestore: '轻触恢复全屏', fullscreenHint: '轻触恢复全屏',
    frameRate: '帧率', automatic: '自动', applying: '正在应用，CarPlay 将在几秒后重新连接',
    videoCodec: '视频编码', codecAuto: '自动（推荐）', codecH264: '强制 H.264', codecH265: '强制 H.265',
    codecHint: '自动：这个浏览器能硬件解码 H.265 时使用 H.265，出问题时自动改用 H.264。强制选项用于测试。',
    codecInUseH264: '当前使用 H.264。', codecInUseH265: '当前使用 H.265。',
    playback: '播放模式', smart: '智能', quality: '质量优先', speed: '速度优先',
    smartHint: '根据网络自动平衡流畅度和延迟。', qualityHint: '画面最流畅，延迟略高。', speedHint: '延迟最低，可能跳过部分画面。',
    picture: '画面', windowSize: '窗口', back: '返回', soundAndMicrophone: '声音与麦克风', plugins: '插件', resetPluginPositions: '重置插件位置', relayVersion: '版本',
    connectingIphone: '正在连接 iPhone', connectingIphoneHint: '请将 iPhone 放在附近，正在建立 CarPlay 连接。',
    authenticating: '正在准备 CarPlay', authenticatingHint: '正在完成 iPhone 认证。',
    joiningNetwork: '正在连接无线网络', joiningNetworkHint: '请保持中继热点或车机网络可用。',
    waitingMedia: '等待 CarPlay 画面', waitingMediaHint: 'iPhone 已连接，正在等待 CarPlay 传输画面。',
    phoneFailed: 'iPhone 连接中断', phoneFailedHint: '请检查 iPhone，并保持中继设备开启以重新连接。', settings: '设置', close: '关闭', sound: '声音', microphone: '麦克风', fit: '画面适配',
    contain: '适应', fill: '填满', fullscreen: '全屏', developer: '开发者模式', about: '关于',
    source: '源代码', version: '网页版本', powered: 'Powered by OpenXPlay.', soundHint: '轻触开启声音',
    code: '输入配对码', codeHint: '输入中继设备上显示的 8 位数字配对码。', incorrect: '配对码错误',
    retryIn: '{seconds} 秒后重试。请检查中继设备和网络。', connect: '连接', connecting: '正在连接', connectingHint: '正在连接中继设备…',
    waiting: '等待 iPhone', waitingHint: '将 iPhone 连接到中继设备以启动 CarPlay。',
    reconnecting: '正在重新连接', reconnectingHint: '请保持中继设备开启并检查网络连接。',
    error: '无法连接中继设备', errorHint: '请开启中继设备并连接同一网络，然后重试。', retry: '重试',
    unsupported: '浏览器不受支持', unsupportedHint: '请换用其他浏览器，并打开中继设备的安全（HTTPS）地址。',
    idle: '尚未请求', capturing: '麦克风已开启', requesting: '正在请求权限…',
    denied: '权限被拒绝或不可用。请在浏览器设置中允许使用麦克风。',
    unavailable: '此浏览器无法使用麦克风。', disabled: '麦克风已关闭',
    projection: 'CarPlay 画面',
    notInUse: '未在使用', listening: '正在聆听', blocked: '已阻止',
    micIdleHint: '仅在 CarPlay 需要语音时使用。', micListeningHint: 'CarPlay 正在使用麦克风。',
    micBlockedHint: '请在此网站的浏览器设置中允许麦克风权限，然后重试。',
    micUnavailableHint: '请通过 HTTPS 使用支持麦克风的浏览器。',
    micRequestHint: '请在浏览器权限提示中选择允许。', enabled: '已开启', off: '已关闭',
    fullscreenUnavailable: '此浏览器不支持', soundBlocked: '声音已暂停', soundBlockedHint: '轻触以允许浏览器播放声音。',
    microphoneDenied: '麦克风已被阻止', microphoneDeniedHint: '请在浏览器设置中允许使用麦克风。',
    playing: 'CarPlay 正在运行', playingHint: '在此屏幕上操作 CarPlay。', paired: '配对码已接受',
    attempt: '第 {attempt} 次重试 · {seconds} 秒后重试。请检查中继设备和网络。',
  },
};
export type TextKey = keyof typeof strings.en;
export function playbackHint(mode: PlaybackMode): TextKey {
  return mode === 'QUALITY' ? 'qualityHint' : mode === 'SPEED' ? 'speedHint' : 'smartHint';
}
export type StatusState = 'code' | 'incorrect' | 'connecting' | 'waiting' | 'reconnecting' | 'error' | 'unsupported' | 'playing' | 'soundBlocked' | 'microphoneDenied' | 'connectingIphone' | 'authenticating' | 'joiningNetwork' | 'waitingMedia' | 'phoneFailed';
export function statusCard(state: StatusState): { headline: TextKey; subline: TextKey; hidden: boolean } {
  const headline = state;
  const subline: TextKey = state === 'code' || state === 'incorrect' ? 'codeHint' :
    state === 'playing' ? 'playingHint' : `${state}Hint`;
  return { headline, subline, hidden: state === 'playing' };
}
export interface Preferences { videoCodec: VideoCodecPreference; frameRate: FrameRatePreference; playback: PlaybackMode; sound: boolean; microphone: boolean; fit: 'fit' | 'fill'; developer: boolean }
export interface Store { getItem(key: string): string | null; setItem(key: string, value: string): void; removeItem(key: string): void }
export class ReceiverPreferences {
  constructor(private readonly storage: () => Store) {}
  read(key: string): string | null { try { return this.storage().getItem(`oxp.${key}`); } catch { return null; } }
  write(key: string, value: string | null): void {
    try { if (value === null) this.storage().removeItem(`oxp.${key}`); else this.storage().setItem(`oxp.${key}`, value); } catch { }
  }
  load(fragment: URLSearchParams): Preferences {
    this.write('gear', null);
    return { videoCodec: videoCodecPreference(null), frameRate: frameRatePreference(this.read('frameRate')), playback: playbackMode(this.read('playback')), sound: this.read('sound') !== 'false', microphone: this.read('microphone') !== 'false',
      fit: this.read('fit') === 'fill' ? 'fill' : 'fit', developer: fragment.has('debug') || this.read('developer') === 'true' };
  }
  save(preferences: Preferences): void {
    for (const [key, value] of Object.entries(preferences)) this.write(key, String(value));
  }
}
export class Pairing {
  token: string | null;
  constructor(private readonly preferences: ReceiverPreferences, fragment: URLSearchParams) {
    this.token = fragment.get('k') || preferences.read('code');
  }
  submit(code: string): boolean {
    if (!/^\d{8}$/.test(code)) return false;
    this.token = code; return true;
  }
  welcome(token = this.token ?? ''): void { if (token) this.preferences.write('code', token); }
  rejected(token = this.token ?? ''): 'code' | 'incorrect' { this.token = null; this.preferences.write('code', null); return token ? 'incorrect' : 'code'; }
}

/** Keep protocol terminology behind developer mode and map it to one useful next step. */
export function protocolStatus(protocol: string, playing: boolean): StatusState {
  const startup: Record<string, StatusState> = {
    'startup/connecting': 'connectingIphone', 'startup/authenticating': 'authenticating',
    'startup/joining-network': 'joiningNetwork', 'startup/waiting-media': 'waitingMedia',
    'startup/failed': 'phoneFailed', 'startup/disconnected': 'phoneFailed',
  };
  if (Object.hasOwn(startup, protocol)) return startup[protocol];
  if (protocol === 'ACTIVE') return playing ? 'playing' : 'waiting';
  if (protocol === 'CONNECTING' || protocol === 'ENDED') return 'waiting';
  if (protocol === 'RECONNECTING' || protocol.startsWith('Disconnected')) return 'reconnecting';
  if (protocol === 'Connecting…') return 'connecting';
  if (/Unsupported codec|WebCodecs is unavailable|Decoder unavailable/.test(protocol)) return 'unsupported';
  return 'error';
}

export function microphoneStatus(state: TextKey): { label: TextKey; hint: TextKey; retry: boolean } {
  if (state === 'capturing') return { label: 'listening', hint: 'micListeningHint', retry: false };
  if (state === 'denied') return { label: 'blocked', hint: 'micBlockedHint', retry: true };
  if (state === 'unavailable') return { label: 'blocked', hint: 'micUnavailableHint', retry: false };
  return { label: 'notInUse', hint: state === 'requesting' ? 'micRequestHint' : 'micIdleHint', retry: false };
}
export function applyReducedMotion(target: { classList: { toggle(name: string, force: boolean): unknown } }, reduce: boolean): void {
  target.classList.toggle('reduced-motion', reduce);
}

/** Choose one audio action according to the visible video, independently of protocol status. */
export function soundPrompt(playing: boolean, enabled: boolean, audioState: string | undefined, audioActive = true): 'pill' | 'card' | null {
  return enabled && audioActive && audioState === 'suspended' ? playing ? 'pill' : 'card' : null;
}
