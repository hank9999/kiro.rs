import axios from 'axios'

export interface CaptchaResponse {
  token: string
  image: string
}

export interface LoginRequest {
  apiKey: string
  captchaToken: string
  captchaAnswer: string
}

export interface LoginResponse {
  success: boolean
  message: string
  token: string
  expiresIn: number
}

export async function getCaptcha(): Promise<CaptchaResponse> {
  const { data } = await axios.get<CaptchaResponse>('/api/admin/auth/captcha')
  return data
}

export async function login(req: LoginRequest): Promise<LoginResponse> {
  const { data } = await axios.post<LoginResponse>('/api/admin/auth/login', req)
  return data
}
