import { forwardRef } from 'react';
import { Search } from 'lucide-react';

export const SearchInput = forwardRef<HTMLInputElement, {
  value: string;
  onChange: (value: string) => void;
  placeholder: string;
  label?: string;
}>(function SearchInput({
  value,
  onChange,
  placeholder,
  label = placeholder,
}, ref) {
  return (
    <label className="search-input">
      <Search size={16} />
      <input ref={ref} value={value} onChange={(event) => onChange(event.target.value)} placeholder={placeholder} aria-label={label} />
    </label>
  );
});
