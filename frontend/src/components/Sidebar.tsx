import { useState } from 'react';
import { Bot, MessageSquare, Settings, Globe, Users, Menu } from 'lucide-react';
import type { AppSettings, AvatarIcon, NavKey } from '../types';
import { useI18n } from '../i18n';
import { AvatarPicker, AvatarBadge } from './AvatarPicker';

interface SidebarProps {
  activeNav: NavKey;
  settings: AppSettings;
  onNavChange: (nav: NavKey) => void;
  onSettingsSave: (settings: AppSettings) => Promise<AppSettings>;
  onMobileHome: () => void;
}

export function Sidebar({ activeNav, settings, onNavChange, onSettingsSave, onMobileHome }: SidebarProps) {
  const { t, locale, setLocale } = useI18n();
  const [showPicker, setShowPicker] = useState(false);

  function handleAvatarChange(next: { icon: AvatarIcon; color: string }) {
    void onSettingsSave({ ...settings, avatar_icon: next.icon, avatar_color: next.color });
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
            title={t('brand.name')}
            aria-label={t('brand.name')}
            onClick={() => setShowPicker(v => !v)}
          >
            <AvatarBadge icon={settings.avatar_icon} color={settings.avatar_color} size={22} className="circular-avatar-inner" />
          </button>

          {showPicker && (
            <>
              <div className="avatar-picker-backdrop" onClick={() => setShowPicker(false)} />
              <div className="avatar-picker">
                <p className="avatar-picker-title">{locale === 'zh' ? '选择头像样式' : 'Choose Avatar'}</p>
                <AvatarPicker
                  icon={settings.avatar_icon}
                  color={settings.avatar_color}
                  title={locale === 'zh' ? '用户头像' : 'User avatar'}
                  onChange={handleAvatarChange}
                />
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
