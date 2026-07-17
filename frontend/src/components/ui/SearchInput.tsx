import { forwardRef } from 'react';
import { Search } from 'lucide-react';

export const SearchInput = forwardRef<HTMLInputElement, {
  value: string;
  onChange: (value: string) => void;
  placeholder: string;
}>(function SearchInput({
  value,
  onChange,
  placeholder,
}, ref) {
  return (
    <label className="search-input">
      <Search size={16} />
      <input ref={ref} value={value} onChange={(event) => onChange(event.target.value)} placeholder={placeholder} />
    </label>
  );
});
