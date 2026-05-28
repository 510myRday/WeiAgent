import { useState } from 'react';
import { Bot, MessageSquare, Settings, Globe, Users, Menu, Sparkles, Zap, Heart, Star, Smile } from 'lucide-react';
import type { NavKey } from '../types';
import { useI18n } from '../i18n';
import { AVATAR_PRESETS, getSavedAvatarId, saveAvatarId, getPresetById } from '../avatarPresets';
import type { AvatarPreset } from '../avatarPresets';

interface SidebarProps {
  activeNav: NavKey;
  onNavChange: (nav: NavKey) => void;
  onMobileHome: () => void;
}

const ICON_MAP: Record<AvatarPreset['icon'], React.ComponentType<{ size?: number; color?: string }>> = {
  bot: Bot,
  sparkles: Sparkles,
  zap: Zap,
  heart: Heart,
  star: Star,
  smile: Smile,
};

export function Sidebar({ activeNav, onNavChange, onMobileHome }: SidebarProps) {
  const { t, locale, setLocale } = useI18n();
  const [avatarId, setAvatarId] = useState(getSavedAvatarId);
  const [showPicker, setShowPicker] = useState(false);

  const preset = getPresetById(avatarId);
  const IconComp = ICON_MAP[preset.icon];

  function handlePick(id: string) {
    setAvatarId(id);
    saveAvatarId(id);
    setShowPicker(false);
  }

  return (
    <aside className="sidebar app-rail" aria-label={t('sidebar.ariaLabel')}>
      <button
        className="rail-button mobile-rail-toggle"
        type="button"
        onClick={onMobileHome}
        aria-label={t('mobile.showSidebar')}
      >
        <Menu size={20} />
      </button>

      <div className="rail-header">
        <div className="rail-avatar-wrapper">
          <button
            className="rail-avatar circular-avatar"
            type="button"
            title={t('brand.name') + ' - ' + preset.label}
            aria-label={t('brand.name')}
            style={{ background: preset.bg, borderColor: preset.color }}
            onClick={() => setShowPicker(v => !v)}
          >
            <IconComp size={22} color={preset.color} />
          </button>

          {showPicker && (
            <>
              <div className="avatar-picker-backdrop" onClick={() => setShowPicker(false)} />
              <div className="avatar-picker">
                <p className="avatar-picker-title">{locale === 'zh' ? '选择头像样式' : 'Choose Avatar'}</p>
                <div className="avatar-picker-grid">
                  {AVATAR_PRESETS.map(p => {
                    const PIcon = ICON_MAP[p.icon];
                    const isActive = p.id === avatarId;
                    return (
                      <button
                        key={p.id}
                        type="button"
                        className={'avatar-option' + (isActive ? ' active' : '')}
                        style={{ background: p.bg, borderColor: isActive ? p.color : 'transparent' }}
                        onClick={() => handlePick(p.id)}
                        title={p.label}
                      >
                        <PIcon size={20} color={p.color} />
                      </button>
                    );
                  })}
                </div>
              </div>
            </>
          )}
        </div>
      </div>

      <nav className="sidebar-nav">
        <NavButton active={activeNav === 'chat'} onClick={() => onNavChange('chat')} icon={<MessageSquare size={21} />} label={t('nav.chat')} />
        <NavButton active={activeNav === 'agents'} onClick={() => onNavChange('agents')} icon={<Bot size={21} />} label={t('nav.agents')} />
        <NavButton active={activeNav === 'groups'} onClick={() => onNavChange('groups')} icon={<Users size={21} />} label={t('nav.groups')} />
      </nav>

      <div className="sidebar-spacer" />
      <div className="sidebar-footer">
        <NavButton active={activeNav === 'settings'} onClick={() => onNavChange('settings')} icon={<Settings size={21} />} label={t('nav.settings')} />
        <button
          className="rail-button lang-switch"
          type="button"
          onClick={() => setLocale(locale === 'zh' ? 'en' : 'zh')}
          aria-label={locale === 'zh' ? t('lang.switchTo') : t('lang.switchToZh')}
          title={locale === 'zh' ? t('lang.switchTo') : t('lang.switchToZh')}
        >
          <Globe size={17} />
          <span>{locale === 'zh' ? 'EN' : '中'}</span>
        </button>
      </div>
    </aside>
  );
}

function NavButton({ active, onClick, icon, label }: { active: boolean; onClick: () => void; icon: React.ReactNode; label: string }) {
  return (
    <button className={'nav-item rail-button' + (active ? ' active' : '')} type="button" onClick={onClick} title={label} aria-label={label}>
      {icon}
    </button>
  );
}
