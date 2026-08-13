export function createChatDateFormatters(locale: string) {
  const formatLocale = locale === 'zh' ? 'zh-CN' : 'en-US';
  return {
    time: new Intl.DateTimeFormat(formatLocale, { hour: '2-digit', minute: '2-digit' }),
    dateTime: new Intl.DateTimeFormat(formatLocale, {
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
    }),
    timelineDate: new Intl.DateTimeFormat(formatLocale, { month: 'long', day: 'numeric' }),
    timelineDateWithYear: new Intl.DateTimeFormat(formatLocale, {
      year: 'numeric',
      month: 'long',
      day: 'numeric',
    }),
    markerDate: new Intl.DateTimeFormat(formatLocale, { month: '2-digit', day: '2-digit' }),
    markerTime: new Intl.DateTimeFormat(formatLocale, {
      hour: '2-digit',
      minute: '2-digit',
      hour12: false,
    }),
  };
}

export type ChatDateFormatters = ReturnType<typeof createChatDateFormatters>;

export function formatChatTime(value: string, formatters: ChatDateFormatters) {
  return formatters.time.format(new Date(value));
}

export function formatChatDateTime(value: string, formatters: ChatDateFormatters) {
  return formatters.dateTime.format(new Date(value));
}

export function formatChatTimelineTime(value: string, formatters: ChatDateFormatters) {
  const date = new Date(value);
  const now = new Date();
  const isSameYear = date.getFullYear() === now.getFullYear();
  const isSameDay = date.toDateString() === now.toDateString();
  if (isSameDay) return formatChatTime(value, formatters);
  const dateLabel = (isSameYear ? formatters.timelineDate : formatters.timelineDateWithYear).format(date);
  return `${dateLabel} ${formatters.time.format(date)}`;
}
