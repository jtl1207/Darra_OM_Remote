import 'package:flutter/material.dart';

import '../../models/om_auth.dart';

/// 邮箱密码登录页: 未登录时挡住主界面。
class LoginPage extends StatefulWidget {
  const LoginPage({Key? key, this.auth}) : super(key: key);

  /// 登录态实例; 缺省用生产单例, 测试可注入。
  final OmAuth? auth;

  @override
  State<LoginPage> createState() => _LoginPageState();
}

class _LoginPageState extends State<LoginPage> {
  OmAuth get _auth => widget.auth ?? OmAuth.instance;
  final TextEditingController _email = TextEditingController();
  final TextEditingController _password = TextEditingController();
  final FocusNode _passwordFocus = FocusNode();
  String _error = '';
  bool _busy = false;

  @override
  void dispose() {
    _email.dispose();
    _password.dispose();
    _passwordFocus.dispose();
    super.dispose();
  }

  Future<void> _submit() async {
    final mail = _email.text.trim();
    final pwd = _password.text;
    if (mail.isEmpty || pwd.isEmpty) {
      setState(() => _error = '邮箱与密码不能为空');
      return;
    }
    setState(() {
      _busy = true;
      _error = '';
    });
    // 成功后主界面由 OmAuth.loginState 切换, 此处不再手动跳转。
    final ok = await _auth.login(mail, pwd);
    if (!mounted) return;
    setState(() {
      _busy = false;
      if (!ok) _error = '登录失败: 账号或密码错误, 或服务不可达';
    });
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Center(
        child: SingleChildScrollView(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 360),
            child: Padding(
              padding: const EdgeInsets.all(24),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Text(
                    'Darra 运维',
                    textAlign: TextAlign.center,
                    style: Theme.of(context).textTheme.titleLarge,
                  ),
                  const SizedBox(height: 24),
                  TextField(
                    controller: _email,
                    enabled: !_busy,
                    keyboardType: TextInputType.emailAddress,
                    textInputAction: TextInputAction.next,
                    decoration: const InputDecoration(labelText: '邮箱'),
                    onSubmitted: (_) => _passwordFocus.requestFocus(),
                  ),
                  const SizedBox(height: 12),
                  TextField(
                    controller: _password,
                    enabled: !_busy,
                    focusNode: _passwordFocus,
                    obscureText: true,
                    textInputAction: TextInputAction.done,
                    decoration: const InputDecoration(labelText: '密码'),
                    onSubmitted: (_) => _submit(),
                  ),
                  const SizedBox(height: 16),
                  SizedBox(
                    height: 40,
                    child: ElevatedButton(
                      onPressed: _busy ? null : _submit,
                      child: Text(_busy ? '登录中' : '登录'),
                    ),
                  ),
                  const SizedBox(height: 12),
                  if (_error.isNotEmpty)
                    Text(
                      _error,
                      style: const TextStyle(color: Color(0xFFFF5252)),
                    ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}
