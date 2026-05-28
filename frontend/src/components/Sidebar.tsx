import { Bot, MessageSquare, Clock, Settings, Globe } from 'lucide-react';
import type { NavKey } from '../types';
import { useI18n } from '../i18n';

interface SidebarProps {
  activeNav: NavKey;
  sidebarOpen: boolean;
  onNavChange: (nav: NavKey) => void;
  onToggleSidebar: () => void;
}

export function Sidebar({ activeNav, sidebarOpen, onNavChange, onToggleSidebar }: SidebarProps) {
  const { t, locale, setLocale } = useI18n();

  return (
    <aside className={`sidebar${sidebarOpen ? ' open' : ''}`} aria-label={t('sidebar.ariaLabel')}>
      <button
        className="sidebar-toggle"
        type="button"
        onClick={onToggleSidebar}
        aria-label={sidebarOpen ? t('mobile.hideSidebar') : t('mobile.showSidebar')}
      >
        <MessageSquare size={20} />
      </button>

      {sidebarOpen ? (
        <div className="sidebar-overlay" onClick={onToggleSidebar} aria-hidden="true" />
      ) : null}

        <div className="brand-row">
          <div className="brand-mark" aria-hidden="true"><Bot size={22} /></div>
          <div>
            <h1>{t('brand.name')}</h1>
            <p>{t('brand.tagline')}</p>
          </div>
        </div>

        <nav className="sidebar-nav">
          <NavButton active={activeNav === 'chat'} onClick={() => onNavChange('chat')} icon={<MessageSquare size={18} />} label={t('nav.chat')} />
          <NavButton active={activeNav === 'agents'} onClick={() => onNavChange('agents')} icon={<Bot size={18} />} label={t('nav.agents')} />
          <NavButton active={activeNav === 'history'} onClick={() => onNavChange('history')} icon={<Clock size={18} />} label={t('nav.history')} />
          <NavButton active={activeNav === 'settings'} onClick={() => onNavChange('settings')} icon={<Settings size={18} />} label={t('nav.settings')} />
        </nav>

        <div className="sidebar-spacer" />
        <div className="sidebar-footer">
          <button
            className="lang-switch"
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
    <button className={'nav-item' + (active ? ' active' : '')} type="button" onClick={onClick}>
      {icon}
      <span>{label}</span>
    </button>
  );
}

