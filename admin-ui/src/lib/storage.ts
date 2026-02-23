const TOKEN_STORAGE_KEY = 'adminToken'

export const storage = {
  getToken: () => localStorage.getItem(TOKEN_STORAGE_KEY),
  setToken: (token: string) => localStorage.setItem(TOKEN_STORAGE_KEY, token),
  removeToken: () => localStorage.removeItem(TOKEN_STORAGE_KEY),
}
