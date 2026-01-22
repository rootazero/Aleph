import type { LucideIcon } from 'lucide-react';
import {
  Bot,
  Brain,
  Sparkles,
  Server,
  Eye,
  Moon,
  GitBranch,
  Zap,
  Cloud,
  Github,
} from 'lucide-react';

export interface PresetProvider {
  id: string;
  name: string;
  icon: LucideIcon;
  color: string;
  type: 'openai' | 'anthropic' | 'gemini' | 'ollama' | 'custom';
  defaultModel: string;
  description: string;
  baseUrl?: string;
}

export const presetProviders: PresetProvider[] = [
  {
    id: 'openai',
    name: 'OpenAI',
    icon: Bot,
    color: '#10a37f',
    type: 'openai',
    defaultModel: 'gpt-4o',
    description: 'GPT-4o and GPT-3.5 models',
  },
  {
    id: 'anthropic',
    name: 'Anthropic',
    icon: Brain,
    color: '#d97757',
    type: 'anthropic',
    defaultModel: 'claude-3-5-sonnet-20241022',
    description: 'Claude models for analysis and coding',
  },
  {
    id: 'google-gemini',
    name: 'Google Gemini',
    icon: Sparkles,
    color: '#4285f4',
    type: 'gemini',
    defaultModel: 'gemini-2.0-flash',
    description: 'Google multimodal AI models',
  },
  {
    id: 'ollama',
    name: 'Ollama',
    icon: Server,
    color: '#000000',
    type: 'ollama',
    defaultModel: 'llama3.2',
    description: 'Run LLMs locally',
    baseUrl: 'http://localhost:11434',
  },
  {
    id: 'deepseek',
    name: 'DeepSeek',
    icon: Eye,
    color: '#0066cc',
    type: 'openai',
    defaultModel: 'deepseek-chat',
    description: 'DeepSeek AI models',
    baseUrl: 'https://api.deepseek.com',
  },
  {
    id: 'moonshot',
    name: 'Moonshot',
    icon: Moon,
    color: '#ff6b6b',
    type: 'openai',
    defaultModel: 'moonshot-v1-8k',
    description: 'Moonshot long-context models',
    baseUrl: 'https://api.moonshot.cn/v1',
  },
  {
    id: 'openrouter',
    name: 'OpenRouter',
    icon: GitBranch,
    color: '#8b5cf6',
    type: 'openai',
    defaultModel: 'openai/gpt-4o',
    description: 'Access multiple AI models',
    baseUrl: 'https://openrouter.ai/api/v1',
  },
  {
    id: 't8star',
    name: 'T8Star',
    icon: Zap,
    color: '#FF6B35',
    type: 'openai',
    defaultModel: 'gpt-4o',
    description: 'OpenAI-compatible proxy',
    baseUrl: 'https://ai.t8star.cn',
  },
  {
    id: 'azure-openai',
    name: 'Azure OpenAI',
    icon: Cloud,
    color: '#0078d4',
    type: 'openai',
    defaultModel: 'gpt-4o',
    description: 'Microsoft Azure hosted OpenAI',
  },
  {
    id: 'github-copilot',
    name: 'GitHub Copilot',
    icon: Github,
    color: '#24292e',
    type: 'openai',
    defaultModel: 'gpt-4o',
    description: 'GitHub Copilot API',
  },
];
