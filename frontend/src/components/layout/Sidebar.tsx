import { Bot, Globe, MessageSquare, Settings, Users, Wrench } from 'lucide-react';
import type { ReactNode } from 'react';

import type { AppSettings, AvatarIcon, NavKey } from '../../types';
import { useI18n } from '../../i18n';
import { AvatarBadge } from '../AvatarPicker';
import { AvatarPopover, IconButton } from '../ui';

interface SidebarProps {
  activeNav: NavKey;
  settings: AppSettings;
  onNavChange: (nav: NavKey) => void;
  onSettingsSave: (settings: AppSettings) => Promise<AppSettings>;
}

export function Sidebar({ activeNav, settings, onNavChange, onSettingsSave }: SidebarProps) {
  const { t, locale, setLocale } = useI18n();

  function handleAvatarChange(next: { icon: AvatarIcon; color: string }) {
    void onSettingsSave({ ...settings, avatar_icon: next.icon, avatar_color: next.color });
  }

  function handleLocaleToggle() {
    const nextLocale = locale === 'zh' ? 'en' : 'zh';
    setLocale(nextLocale);
    void onSettingsSave({ ...settings, language: nextLocale });
  }

  return (
    <aside className="sidebar app-rail" aria-label={t('sidebar.ariaLabel')}>
      <div className="rail-header">
        <AvatarPopover
          icon={settings.avatar_icon}
          color={settings.avatar_color}
          title={t('sidebar.avatarPreview')}
          pickerTitle={t('agentConfig.avatarChange')}
          onChange={handleAvatarChange}
          className="rail-avatar-popover"
          trigger={() => (
            <button className="rail-avatar-button" type="button" title={t('brand.name')} aria-label={t('brand.name')}>
              <AvatarBadge icon={settings.avatar_icon} color={settings.avatar_color} size={22} className="avatar-badge avatar-badge-rail" />
            </button>
          )}
        />
        <div className="rail-brand-copy" aria-hidden="true">
          <strong>{t('brand.name')}</strong>
          <span>{t('brand.tagline')}</span>
        </div>
      </div>

      <nav className="sidebar-nav">
        <NavButton active={activeNav === 'chat'} onClick={() => onNavChange('chat')} icon={<MessageSquare size={21} />} label={t('nav.chat')} />
        <NavButton active={activeNav === 'agents'} onClick={() => onNavChange('agents')} icon={<Bot size={21} />} label={t('nav.agents')} />
        <NavButton active={activeNav === 'groups'} onClick={() => onNavChange('groups')} icon={<Users size={21} />} label={t('nav.groups')} />
        <NavButton active={activeNav === 'tools'} onClick={() => onNavChange('tools')} icon={<Wrench size={21} />} label={t('nav.tools')} />
        <NavButton active={activeNav === 'settings'} onClick={() => onNavChange('settings')} icon={<Settings size={21} />} label={t('nav.settings')} />
      </nav>

      <div className="sidebar-spacer" />
      <div className="sidebar-footer">
        <IconButton
          className="lang-switch"
          variant="rail"
          type="button"
          onClick={handleLocaleToggle}
          label={locale === 'zh' ? t('lang.switchTo') : t('lang.switchToZh')}
        >
          <Globe size={17} />
          <span>{locale === 'zh' ? 'EN' : '中'}</span>
        </IconButton>
      </div>
    </aside>
  );
}

function NavButton({ active, onClick, icon, label }: { active: boolean; onClick: () => void; icon: ReactNode; label: string }) {
  return (
    <IconButton variant="rail" active={active} type="button" onClick={onClick} label={label} aria-current={active ? 'page' : undefined}>
      {icon}
      <span className="rail-nav-label">{label}</span>
    </IconButton>
  );
}
