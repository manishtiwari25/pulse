import js from '@eslint/js';
import react from 'eslint-plugin-react';
import globals from 'globals';
import tseslint from 'typescript-eslint';

export default tseslint.config(
  { ignores: ['dist/', 'node_modules/'] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ['src/**/*.{ts,tsx}'],
    plugins: { react },
    languageOptions: { globals: globals.browser, parserOptions: { ecmaFeatures: { jsx: true } } },
    settings: { react: { version: '18.0', pragma: 'h' } },
    rules: {
      // ADR-023 §3: the page only ever inserts text; the guard module checks
      // the same rule over this source tree.
      'react/no-danger': 'error',
      'react/jsx-key': 'error',
      'react/jsx-no-target-blank': 'error',
      'react/jsx-no-script-url': 'error',
      'no-restricted-syntax': [
        'error',
        { selector: "MemberExpression[property.name=/^(inner|outer)HTML$/]", message: 'text only: never assign markup (ADR-023 §3)' },
        { selector: "CallExpression[callee.property.name='insertAdjacentHTML']", message: 'text only (ADR-023 §3)' },
      ],
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_' }],
    },
  },
);
