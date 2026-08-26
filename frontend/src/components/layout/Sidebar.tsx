import { BookOpen, Bot, CalendarClock, ChartColumn, Globe, Settings, Wrench } from 'lucide-react';
import type { ReactNode } from 'react';

import type { AppSettings, AvatarIcon, NavKey } from '../../types';
import { useI18n } from '../../i18n';
import { AvatarBadge } from '../AvatarPicker';
import { AvatarPopover, IconButton, useFeedback } from '../ui';
import { userErrorMessage } from '../../utils/userError';

interface SidebarProps {
  activeNav: NavKey;
  chatUnreadCount?: number;
  settings: AppSettings;
  onNavChange: (nav: NavKey) => void;
  onSettingsSave: (settings: AppSettings) => Promise<AppSettings>;
}

export function Sidebar({ activeNav, chatUnreadCount = 0, settings, onNavChange, onSettingsSave }: SidebarProps) {
  const { t, locale, setLocale } = useI18n();
  const { toast } = useFeedback();

  function handleAvatarChange(next: { icon: AvatarIcon; color: string }) {
    void onSettingsSave({ ...settings, avatar_icon: next.icon, avatar_color: next.color })
      .catch((cause) => toast.error(userErrorMessage(cause, {
        summary: t('error.settingsSaveFailed'),
        recovery: t('error.retryConnection'),
      })));
  }

  async function handleLocaleToggle() {
    const previousLocale = locale;
    const nextLocale = locale === 'zh' ? 'en' : 'zh';
    setLocale(nextLocale);
    try {
      const updated = await onSettingsSave({ ...settings, language: nextLocale });
      setLocale(updated.language);
    } catch (cause) {
      setLocale(previousLocale);
      toast.error(userErrorMessage(cause, {
        summary: t('error.settingsSaveFailed'),
        recovery: t('error.retryConnection'),
      }));
    }
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
        <NavButton
          active={activeNav === 'chat'}
          onClick={() => onNavChange('chat')}
          icon={<ChatBubbleMoreIcon />}
          label={t('nav.chat')}
          showUnread={chatUnreadCount > 0}
        />
        <NavButton active={activeNav === 'agents'} onClick={() => onNavChange('agents')} icon={<Bot size={21} />} label={t('nav.agents')} />
        <NavButton active={activeNav === 'tools'} onClick={() => onNavChange('tools')} icon={<Wrench size={21} />} label={t('nav.tools')} />
        <NavButton active={activeNav === 'skills'} onClick={() => onNavChange('skills')} icon={<BookOpen size={21} />} label={t('nav.skills')} />
        <NavButton active={activeNav === 'monitor'} onClick={() => onNavChange('monitor')} icon={<ChartColumn size={21} />} label={t('nav.monitor')} />
        <NavButton active={activeNav === 'scheduling'} onClick={() => onNavChange('scheduling')} icon={<CalendarClock size={21} />} label={t('nav.scheduling')} />
        <NavButton active={activeNav === 'settings'} onClick={() => onNavChange('settings')} icon={<Settings size={21} />} label={t('nav.settings')} />
      </nav>

      <div className="sidebar-spacer" />
      <div className="sidebar-footer">
        <IconButton
          className="lang-switch"
          variant="rail"
          type="button"
          onClick={() => void handleLocaleToggle()}
          label={locale === 'zh' ? t('lang.switchTo') : t('lang.switchToZh')}
        >
          <Globe size={17} />
          <span>{locale === 'zh' ? 'EN' : '中'}</span>
        </IconButton>
      </div>
    </aside>
  );
}

function ChatBubbleMoreIcon() {
  return (
    <svg width="21" height="21" viewBox="0 0 24 24" fill="none" aria-hidden="true" focusable="false">
      <path
        d="M8.9 18.3 5.2 20l.7-3.1C3.5 15.5 2 13.4 2 10.8 2 6.5 6.3 3.5 12 3.5s10 3 10 7.3-4.3 8-10 8c-1.1 0-2.1-.2-3.1-.5Z"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <circle cx="8" cy="10.8" r="1" fill="currentColor" />
      <circle cx="12" cy="10.8" r="1" fill="currentColor" />
      <circle cx="16" cy="10.8" r="1" fill="currentColor" />
    </svg>
  );
}

function NavButton({
  active,
  className,
  onClick,
  icon,
  label,
  showUnread = false,
}: {
  active: boolean;
  className?: string;
  onClick: () => void;
  icon: ReactNode;
  label: string;
  showUnread?: boolean;
}) {
  return (
    <IconButton className={className} variant="rail" active={active} type="button" onClick={onClick} label={label} aria-current={active ? 'page' : undefined}>
      {icon}
      {showUnread ? <span className="nav-unread-dot" aria-label={label} title={label} /> : null}
      <span className="rail-nav-label">{label}</span>
    </IconButton>
  );
}
